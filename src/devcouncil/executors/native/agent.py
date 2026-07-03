from typing import List, Dict, Any
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

class ToolCall(BaseModel):
    tool: str
    args: Dict[str, Any]

class AgentAction(BaseModel):
    thought: str
    tool_calls: List[ToolCall] = []
    finish: bool = False

class NativeAgent(Executor):
    def __init__(self, router: ModelRouter, task_runner: TaskRunner):
        self.router = router
        self.task_runner = task_runner
        # ContextBuilder is retained only for the cheap list_files file listing; the
        # implementation context itself uses the budgeted PromptBuilder so the native
        # executor gets the same repo-map orientation, symbol outlines, dependents and
        # context-window budgeting as the CLI executors (rather than a flat JSON dump).
        self.context_builder = ContextBuilder(task_runner.project_root)
        self.prompt_builder = PromptBuilder(task_runner.project_root)

    def run_task(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        """Run the preview native executor behind the normal synchronous executor contract."""
        root = self.task_runner.project_root
        log_step(
            f"executor/native: starting task {task.id}",
            project_root=root,
            task_id=task.id,
        )
        result = asyncio.run(self._run_task_async(task, requirements))
        log_step(
            f"executor/native: finished task {task.id}",
            project_root=root,
            task_id=task.id,
            success=result.success,
        )
        return result

    async def _run_task_async(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        logger.info("Native agent starting for %s (max_steps=%d)", task.id, MAX_AGENT_STEPS)
        console.print(f"Starting [bold]Native Executor[/bold] for task {task.id}...")
        console.print("[yellow]Native executor is preview quality; DevCouncil verification remains the completion gate.[/yellow]")
        
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
                logger.info("Native agent signaled completion for %s at step %d", task.id, step + 1)
                console.print("[green]Native agent signaled completion.[/green]")
                return ExecutionResult(success=True, message="Agent signaled completion; pending DevCouncil verification")

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
                            # diff. Routes through write_file, which enforces the same
                            # planned-files permission check — no widening of scope.
                            self.task_runner.write_file(
                                tool_call.args["path"], tool_call.args["content"], task
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
                            self.task_runner.apply_patch(patch, task)
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
