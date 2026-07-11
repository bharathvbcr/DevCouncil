from typing import Any, Coroutine, Dict, List, Optional, Tuple, TypeVar
import asyncio
import logging
from rich.console import Console
from pydantic import BaseModel
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.execution.executor import Executor, ExecutionResult
from devcouncil.llm.router import ModelRouter, StructuredOutputError
from devcouncil.execution.task_runner import TaskRunner
from devcouncil.execution.context_builder import ContextBuilder
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.execution.paths import resolve_project_path
from devcouncil.app.errors import ExecutionError
from devcouncil.telemetry.stages import log_step

console = Console()
logger = logging.getLogger(__name__)

# Resilience bounds for the preview native loop.
MAX_AGENT_STEPS = 10
MAX_STRUCTURED_FAILURES = 2          # model can't produce a valid action -> give up cleanly
MAX_CONSECUTIVE_PATCH_FAILURES = 3   # stop spinning on a patch the model can't fix
# How many times the agent may signal "finish", get BLOCKED by verification, and be
# handed the shared next-actions repair contract before we stop the closed loop. Mirrors
# the bounded self-repair budget the MCP/dev-go loops apply so native cannot spin forever.
MAX_VERIFY_ROUNDS = 3

_T = TypeVar("_T")

class ToolCall(BaseModel):
    tool: str
    args: Dict[str, Any]

class AgentAction(BaseModel):
    thought: str
    tool_calls: List[ToolCall] = []
    finish: bool = False

class NativeAgent(Executor):
    def __init__(self, router: ModelRouter, task_runner: TaskRunner, *, sandbox: str = "local"):
        self.router = router
        self.task_runner = task_runner
        # Writes and verification are now routed through the SAME lease-gated policy path
        # the MCP surface uses (execution/gated_write.py + HookPolicy, verification via the
        # shared verify payload), so the native executor no longer bypasses the lease/scope
        # gate with direct TaskRunner writes. task_runner is retained for run_command (which
        # already honors execution.command_timeout).
        self.project_root = task_runner.project_root
        self.sandbox = sandbox
        # ContextBuilder is retained only for the cheap list_files file listing; the
        # implementation context itself uses the budgeted PromptBuilder so the native
        # executor gets the same repo-map orientation, symbol outlines, dependents and
        # context-window budgeting as the CLI executors (rather than a flat JSON dump).
        self.context_builder = ContextBuilder(task_runner.project_root)
        self.prompt_builder = PromptBuilder(task_runner.project_root)
        self._lease_token: Optional[str] = None

    def run_task(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        """Run the preview native executor behind the normal synchronous executor contract."""
        root = self.task_runner.project_root
        log_step(
            f"executor/native: starting task {task.id}",
            project_root=root,
            task_id=task.id,
        )
        result = self._run_coroutine_from_sync(self._run_task_async(task, requirements))
        log_step(
            f"executor/native: finished task {task.id}",
            project_root=root,
            task_id=task.id,
            success=result.success,
        )
        return result

    @staticmethod
    def _run_coroutine_from_sync(coro: Coroutine[Any, Any, _T]) -> _T:
        """Run async work from sync code without breaking nested event loops.

        ``asyncio.run()`` fails when a loop is already running (pytest-asyncio,
        Jupyter). With no running loop we use ``asyncio.run()``; otherwise we run
        the coroutine on a fresh loop in a worker thread.
        """
        import concurrent.futures

        try:
            asyncio.get_running_loop()
        except RuntimeError:
            return asyncio.run(coro)
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
            return executor.submit(asyncio.run, coro).result()

    # ------------------------------------------------------------------ lease-gated I/O
    def _acquire_lease(self, task: Task) -> dict:
        """Acquire a task lease so writes/verify run through the shared gated path."""
        from devcouncil.execution.lease_ops import checkout_task_payload

        return checkout_task_payload(self.project_root, task_id=task.id, client_id="native")

    def _release_lease(self, task: Task, lease_token: str) -> None:
        from devcouncil.execution.lease_ops import release_task_payload

        try:
            release_task_payload(self.project_root, task_id=task.id, lease_token=lease_token)
        except Exception as exc:  # a failed release only costs a TTL wait; never fail the run
            logger.debug("Native lease release failed for %s: %s", task.id, exc)

    def _gated_apply_patch(self, task: Task, patch: str) -> None:
        """Apply a unified diff through execution/gated_write.py (lease + scope + HookPolicy).

        Raises ExecutionError on rejection so the loop's existing patch-failure handling
        (retry guidance, consecutive-failure abort) applies unchanged.
        """
        from devcouncil.execution.gated_write import apply_patch_payload

        payload = apply_patch_payload(
            self.project_root,
            task_id=task.id,
            lease_token=self._lease_token or "",
            unified_diff=patch,
        )
        if not payload.get("ok"):
            raise ExecutionError(self._write_rejection_reason(payload))

    def _gated_write_file(self, task: Task, path: str, content: str) -> None:
        """Write a whole file through execution/gated_write.py (lease + scope + HookPolicy)."""
        from devcouncil.execution.gated_write import write_file_payload

        payload = write_file_payload(
            self.project_root,
            task_id=task.id,
            lease_token=self._lease_token or "",
            rel_path=path,
            content=content,
        )
        if not payload.get("ok"):
            raise ExecutionError(self._write_rejection_reason(payload))

    @staticmethod
    def _write_rejection_reason(payload: dict) -> str:
        rejected = payload.get("rejected_files") or []
        if rejected:
            first = rejected[0]
            return f"Write to {first.get('path')} rejected: {first.get('reason')}"
        return payload.get("error") or "Write rejected by the lease/scope gate."

    def _verify_task(
        self, task: Task, requirements: List[Requirement]
    ) -> Tuple[bool, List[dict], List[dict]]:
        """Verify through the shared surface and return (passed, blocking, advisory).

        Local sandbox uses the same ``verify_task_payload`` the MCP verify loop uses, so the
        ``split_next_actions`` repair contract is byte-for-byte identical across surfaces.
        docker/nix run through ``verification/sandbox.py`` for coding-CLI parity.
        """
        if self.sandbox in {"docker", "nix"}:
            return self._verify_via_sandbox(task, requirements)
        from devcouncil.execution.task_gate_ops import verify_task_payload

        payload = verify_task_payload(
            self.project_root,
            task_id=task.id,
            lease_token=self._lease_token or "",
            sandbox="local",
        )
        if not payload.get("ok"):
            # A verification setup error (e.g. lease lost) is treated as "not passed" with the
            # error surfaced as the single next action, so the loop can react rather than crash.
            return False, [{"action": payload.get("error", "Verification failed to run.")}], []
        return (
            bool(payload.get("passed")),
            list(payload.get("next_actions") or []),
            list(payload.get("advisory_actions") or []),
        )

    def _verify_via_sandbox(
        self, task: Task, requirements: List[Requirement]
    ) -> Tuple[bool, List[dict], List[dict]]:
        """Run the task's expected commands in a docker/nix sandbox (command_timeout-bounded)."""
        from devcouncil.verification.sandbox import get_sandbox

        commands = task.expected_tests or task.allowed_commands
        result = get_sandbox(self.sandbox, self.project_root).run(task, commands, requirements)
        if result.status == "unsupported":
            reason = f"Sandbox '{self.sandbox}' is unavailable in this environment."
            return False, [{"action": reason}], []
        return result.status == "passed", [], []

    @staticmethod
    def _format_next_actions(blocking: List[dict], advisory: List[dict]) -> str:
        """Render the shared next-actions contract into the repair message fed back to the
        model — the same guidance an MCP agent reads from devcouncil_verify_task."""
        lines = ["[Verification BLOCKED] The change did not pass. Address these before finishing:"]
        for i, action in enumerate(blocking, 1):
            text = action.get("action") or action.get("gap_type") or "Resolve the blocking gap."
            loc = action.get("file")
            if loc and action.get("line"):
                loc = f"{loc}:{action['line']}"
            suffix = f" ({loc})" if loc else ""
            cmd = action.get("suggested_command")
            cmd_txt = f" Suggested command: {cmd}" if cmd else ""
            lines.append(f"{i}. {text}{suffix}{cmd_txt}")
        if advisory:
            lines.append("Advisory (non-blocking) signals worth addressing:")
            for action in advisory:
                lines.append(f"- {action.get('action') or action.get('gap_type')}")
        lines.append(
            "Apply the fix through apply_patch, then set finish=true again to re-verify."
        )
        return "\n".join(lines)

    async def _run_task_async(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        logger.info("Native agent starting for %s (max_steps=%d)", task.id, MAX_AGENT_STEPS)
        console.print(f"Starting [bold]Native Executor[/bold] for task {task.id}...")
        console.print("[yellow]Native executor is preview quality; DevCouncil verification remains the completion gate.[/yellow]")

        # Acquire a lease so every write and verify runs through the same lease/scope gate
        # as the MCP surface. Without a lease the gated write path refuses the write.
        lease = self._acquire_lease(task)
        if not lease.get("ok"):
            message = lease.get("error", "Could not acquire a task lease.")
            logger.error("Native agent could not lease %s: %s", task.id, message)
            console.print(f"[red]Native agent could not acquire a lease for {task.id}: {message}[/red]")
            return ExecutionResult(success=False, message=f"Lease unavailable: {message}")
        self._lease_token = lease["lease_token"]
        try:
            return await self._run_agent_loop(task, requirements)
        finally:
            token = self._lease_token
            self._lease_token = None
            if token:
                self._release_lease(task, token)

    async def _run_agent_loop(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        # 1. Gather rich context (budgeted; includes repo-map orientation + symbol outlines)
        context_block = self.prompt_builder.build_task_prompt(task, requirements)
        from devcouncil.planning.correction_manifest import repair_prompt_prefix

        prefix = repair_prompt_prefix(self.task_runner.project_root, task.id)
        correction_block = f"\n{prefix}" if prefix else ""

        system_prompt = f"""
You are the DevCouncil Native Agent. Your goal is to implement the provided task.
Current Project Context:
{context_block}
{correction_block}

You have access to the following tools:
- read_file(path: str)
- list_files()
- apply_patch(patch: str)  OR  apply_patch(path: str, content: str) as a fallback when a valid unified diff cannot be produced
- run_command(command: str)

Rules:
1. You can only write to files or apply patches to files listed in the task's 'planned_files'.
2. You can only run commands listed in the task's 'allowed_commands'.
3. Use 'thought' to explain your reasoning.
4. Set 'finish' to true when you believe the task is complete and verified.
"""
        messages = [{"role": "system", "content": system_prompt}]

        # Initial task prompt
        messages.append({"role": "user", "content": f"Begin implementing task {task.id} based on the context provided."})

        # Bounded tool loop. Counters let us fail a single task cleanly instead of
        # crashing the whole run (structured-output faults) or spinning on an
        # unfixable patch.
        structured_failures = 0
        consecutive_patch_failures = 0
        verify_rounds = 0
        for step in range(MAX_AGENT_STEPS):
            try:
                action = await self.router.complete_structured(
                    role="native_agent",
                    messages=messages,
                    schema=AgentAction,
                )
            except StructuredOutputError as exc:
                # The model could not produce a valid action even after healing/retry.
                # native_agent has no fallback by design, so handle it here rather than
                # letting it propagate and abort the entire `dev go` run.
                structured_failures += 1
                logger.warning("Native agent step %d: unparseable action (%d/%d): %s", step + 1, structured_failures, MAX_STRUCTURED_FAILURES, exc)
                console.print(f"[red]Native agent could not parse a valid action: {exc}[/red]")
                if structured_failures >= MAX_STRUCTURED_FAILURES:
                    logger.error("Native agent giving up on %s after %d unparseable responses", task.id, structured_failures)
                    return ExecutionResult(
                        success=False,
                        message=f"Native agent gave up after {structured_failures} unparseable responses.",
                    )
                messages.append({
                    "role": "user",
                    "content": (
                        "[System] Your previous response was not valid JSON for the "
                        "AgentAction schema. Reply with a single valid JSON object only "
                        "(fields: thought, tool_calls, finish) — no prose, no fences."
                    ),
                })
                continue
            structured_failures = 0

            logger.info(
                "Native agent %s step %d/%d: %d tool call(s)%s",
                task.id, step + 1, MAX_AGENT_STEPS, len(action.tool_calls),
                " finish=True" if action.finish else "",
            )
            console.print(f"\n[bold]Step {step+1}:[/bold] {action.thought}")

            # Record the agent's own turn so subsequent steps see what it already did.
            # Without this the model only sees tool RESULTS, not its prior actions, and
            # tends to repeat itself and never converge within the step budget.
            messages.append({"role": "assistant", "content": action.model_dump_json()})

            if action.finish:
                # Closed loop: instead of trusting the agent's self-report, verify through the
                # same gate as MCP and, when BLOCKED, feed the shared next-actions repair
                # contract back so the agent can self-repair and re-verify.
                logger.info("Native agent signaled completion for %s at step %d; verifying", task.id, step + 1)
                console.print("[green]Native agent signaled completion; verifying...[/green]")
                # verify_task_payload runs the verifier via asyncio.run(); run it in a worker
                # thread so it gets its own event loop instead of nesting inside this one.
                passed, blocking, advisory = await asyncio.to_thread(
                    self._verify_task, task, requirements
                )
                if passed:
                    logger.info("Native agent %s verified after %d repair round(s)", task.id, verify_rounds)
                    console.print("[green]Verification passed.[/green]")
                    return ExecutionResult(success=True, message="Native agent completed and verification passed.")
                verify_rounds += 1
                if verify_rounds >= MAX_VERIFY_ROUNDS:
                    logger.error("Native agent %s still blocked after %d verify round(s)", task.id, verify_rounds)
                    return ExecutionResult(
                        success=False,
                        message=(
                            f"Verification still blocked after {verify_rounds} repair round(s): "
                            f"{len(blocking)} blocking gap(s)."
                        ),
                    )
                console.print(f"[yellow]Verification blocked ({len(blocking)} gap(s)); handing back repair guidance.[/yellow]")
                messages.append({"role": "user", "content": self._format_next_actions(blocking, advisory)})
                continue

            if not action.tool_calls:
                # No action and not finished — nudge instead of silently burning a step.
                messages.append({"role": "user", "content": (
                    "[System] You produced no tool_calls and did not finish. Call a tool "
                    "(read_file/list_files/apply_patch/run_command) to make progress, or set "
                    "finish=true if the task is complete."
                )})
                continue

            for tool_call in action.tool_calls:
                result_summary = ""
                logger.debug("Native agent tool call: %s args=%s", tool_call.tool, list(tool_call.args))
                try:
                    if tool_call.tool == "read_file":
                        path = tool_call.args["path"]
                        resolved = resolve_project_path(self.task_runner.project_root, path)
                        # Security: block reading sensitive files
                        sensitive_patterns = {".env", ".pem", ".key", "credentials", "secrets"}
                        path_lower = path.lower()
                        if any(s in path_lower for s in sensitive_patterns):
                            raise PermissionError(f"Reading sensitive file blocked: {path}")
                        content = resolved.read_text(encoding="utf-8")
                        if len(content) > 8000:
                            content = content[:8000] + "\n[truncated]"
                        result_summary = f"File content of {path}:\n{content}"
                    elif tool_call.tool == "list_files":
                        # We use the internal helper but return limited list
                        files = self.context_builder.get_structure_summary()
                        result_summary = f"Found {len(files)} files in repository."
                    elif tool_call.tool == "write_file":
                        raise PermissionError("write_file is disabled for the native executor; use apply_patch.")
                    elif tool_call.tool == "apply_patch":
                        if "path" in tool_call.args and "content" in tool_call.args:
                            # Fallback for when the model can't produce a valid unified
                            # diff. Routes through the lease-gated write path, which enforces
                            # the same planned-files/scope policy — no widening of scope.
                            self._gated_write_file(
                                task, tool_call.args["path"], tool_call.args["content"]
                            )
                            consecutive_patch_failures = 0
                            result_summary = f"Wrote {tool_call.args['path']} via path+content fallback."
                        else:
                            patch = tool_call.args.get("patch", "")
                            if not patch or not patch.strip():
                                raise ExecutionError(
                                    "Empty patch. Provide a unified git diff beginning with "
                                    "'diff --git a/<path> b/<path>', then '--- a/<path>' (or "
                                    "'--- /dev/null' for a new file), '+++ b/<path>', and '@@' hunks."
                                )
                            self._gated_apply_patch(task, patch)
                            consecutive_patch_failures = 0
                            result_summary = "Successfully applied patch."
                    elif tool_call.tool == "run_command":
                        cmd_result = self.task_runner.run_command(tool_call.args["command"], task)
                        result_summary = f"Command finished with exit code {cmd_result.exit_code}."
                    else:
                        raise ValueError(f"Unknown tool: {tool_call.tool}")

                    messages.append({"role": "user", "content": f"[Tool Result] '{tool_call.tool}': {result_summary}"})
                except Exception as e:
                    logger.warning("Native agent tool %s failed for %s: %s", tool_call.tool, task.id, e)
                    console.print(f"[red]Error executing tool {tool_call.tool}: {e}[/red]")
                    if tool_call.tool == "apply_patch":
                        consecutive_patch_failures += 1
                        if consecutive_patch_failures >= MAX_CONSECUTIVE_PATCH_FAILURES:
                            logger.error("Native agent giving up on %s after %d consecutive patch failures", task.id, consecutive_patch_failures)
                            return ExecutionResult(
                                success=False,
                                message=(
                                    f"Native agent failed to apply a patch "
                                    f"{consecutive_patch_failures} times in a row."
                                ),
                            )
                        messages.append({"role": "user", "content": (
                            f"[Tool Error] 'apply_patch' failed: {e}\n"
                            "Re-read the target file, match the existing context lines EXACTLY, "
                            "and do NOT resubmit the same patch. If you cannot produce a valid "
                            "unified diff, call apply_patch with 'path' and 'content' instead to "
                            "write the whole file."
                        )})
                    else:
                        messages.append({"role": "user", "content": f"[Tool Error] '{tool_call.tool}' failed: {e}"})

        logger.warning("Native agent reached max step limit (%d) for %s", MAX_AGENT_STEPS, task.id)
        console.print("[red]Native agent reached maximum step limit.[/red]")
        return ExecutionResult(success=False, message="Reached maximum step limit")
