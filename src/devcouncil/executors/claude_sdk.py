"""In-process Claude executor backed by the Claude Agent SDK.

Unlike :class:`CodingCliExecutor`, which shells out to ``claude -p`` and can only enforce
scope *after* the subprocess has already written to disk, this executor arbitrates every
tool call live through the SDK's ``can_use_tool`` callback. The callback is backed by the
same :class:`HookPolicy` the shell write-gate uses, so an out-of-scope write or command is
denied *before it happens* — real containment that, unlike the shell PreToolUse gate, does
not fail-closed on an interactive session (the decision is evaluated against the run's own
task, not a globally-resolved lease).

The Claude Agent SDK is an OPTIONAL dependency: it is imported lazily so nothing else in
DevCouncil requires it, and a missing install produces an actionable error rather than an
import-time crash. Wire it up with ``pip install claude-agent-sdk``.
"""

from __future__ import annotations

import importlib
import logging
from pathlib import Path
from typing import Any, Coroutine, List, Optional, TypeVar

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.execution.executor import ExecutionResult, Executor
from devcouncil.execution.hook_policy import HookDecision, HookPolicy
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.executors.transient_retry import transient_error_in_text
from devcouncil.telemetry.stages import log_step

logger = logging.getLogger(__name__)

_T = TypeVar("_T")


class ClaudeSdkExecutor(Executor):
    """Run a DevCouncil task through the Claude Agent SDK with lease-aware gating."""

    def __init__(
        self,
        project_root: Path | str,
        *,
        active_task: Task | None = None,
        model: str | None = None,
        advisor_model: str | None = None,
        permission_mode: str = "default",
        env: dict[str, str] | None = None,
    ):
        # NOTE: default (not acceptEdits). acceptEdits auto-approves file edits, which would
        # skip the can_use_tool callback entirely and defeat the lease gate; "default" routes
        # every write/command through our callback so scope is actually enforced.
        self.project_root = Path(project_root)
        # The task whose scope authorizes writes/commands. Passed explicitly so the gate is
        # evaluated against THIS run's task rather than a globally-resolved lease (which is
        # why this can enforce interactively where the shell PreToolUse gate cannot).
        self.active_task = active_task
        self.model = model
        self.advisor_model = (advisor_model or "").strip() or None
        self.permission_mode = permission_mode
        # Extra environment for the SDK's underlying Claude Code process. Lets a run
        # target an alternative Anthropic-compatible endpoint (ANTHROPIC_BASE_URL /
        # ANTHROPIC_AUTH_TOKEN, e.g. a local proxy) — same knob the subprocess
        # executor exposes via per-profile ``env`` overrides.
        self.env = dict(env) if env else {}
        self.policy = HookPolicy(project_root=self.project_root)
        self.last_agent_session_id: str | None = None
        # (tool_name, reason) for every call the gate denied — surfaced in the result.
        self.denials: list[tuple[str, str]] = []

    # -- policy ---------------------------------------------------------------------

    def permission_decision(self, tool_name: str, tool_input: dict[str, Any]) -> HookDecision:
        """Lease-aware allow/deny for a single tool call, reusing DevCouncil's HookPolicy.

        Pure and side-effect free (bar reading repo policy) so it is unit-testable on its
        own: the same call the SDK would make, evaluated against this run's task."""
        call_data = {"tool_name": tool_name, "tool_input": tool_input or {}}
        return self.policy.evaluate(call_data, self.active_task)

    # -- SDK loading ----------------------------------------------------------------

    @staticmethod
    def _load_sdk():
        try:
            return importlib.import_module("claude_agent_sdk")
        except ModuleNotFoundError as exc:
            raise RuntimeError(
                "The Claude Agent SDK is not installed. Install it with "
                "`pip install claude-agent-sdk` to use the claude-sdk executor."
            ) from exc

    @staticmethod
    def _allow(sdk, tool_input: dict[str, Any]):
        """Build the SDK's 'allow' permission result, preferring its typed class."""
        allow_cls = getattr(sdk, "PermissionResultAllow", None)
        if allow_cls is not None:
            try:
                return allow_cls(updated_input=tool_input)
            except TypeError:
                return allow_cls()
        # Documented plain-object contract as a fallback.
        return {"behavior": "allow", "updatedInput": tool_input}

    @staticmethod
    def _deny(sdk, reason: str):
        deny_cls = getattr(sdk, "PermissionResultDeny", None)
        if deny_cls is not None:
            try:
                return deny_cls(message=reason)
            except TypeError:
                return deny_cls(reason)
        return {"behavior": "deny", "message": reason}

    # -- run ------------------------------------------------------------------------

    def run_task(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        # A task can be passed at construction (for the gate) or here; the run argument wins.
        if task is not None:
            self.active_task = task
        log_step(
            f"executor/claude-sdk: starting task {task.id}",
            project_root=self.project_root,
            task_id=task.id,
        )
        try:
            result = self._run_coroutine_from_sync(self._run_async(task, requirements))
            log_step(
                f"executor/claude-sdk: finished task {task.id}",
                project_root=self.project_root,
                task_id=task.id,
                success=result.success,
            )
            return result
        except RuntimeError as exc:
            # Missing SDK or a nested event loop — report cleanly, never crash the caller.
            logger.error("claude-sdk executor could not run %s: %s", getattr(task, "id", "?"), exc)
            log_step(
                f"executor/claude-sdk: finished task {getattr(task, 'id', '?')}",
                project_root=self.project_root,
                task_id=getattr(task, "id", None),
                success=False,
                error=str(exc),
            )
            return ExecutionResult(success=False, message=str(exc))

    @staticmethod
    def _run_coroutine_from_sync(coro: Coroutine[Any, Any, _T]) -> _T:
        """Run an async coroutine from sync code, including nested-loop contexts.

        ``asyncio.run()`` raises when a loop is already running (pytest-asyncio,
        Jupyter). With no running loop we use ``asyncio.run()``; otherwise we run
        the coroutine on a fresh loop in a worker thread.
        """
        import asyncio
        import concurrent.futures

        try:
            asyncio.get_running_loop()
        except RuntimeError:
            return asyncio.run(coro)
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
            return executor.submit(asyncio.run, coro).result()

    async def _run_async(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        sdk = self._load_sdk()
        prompt = PromptBuilder(self.project_root).build_task_prompt(task, requirements)
        # Parity with the subprocess executor: a repair run must carry the correction
        # manifest so the model knows what the prior attempt got wrong.
        from devcouncil.planning.correction_manifest import repair_prompt_prefix

        prefix = repair_prompt_prefix(self.project_root, task.id)
        if prefix:
            prompt = f"{prefix}{prompt}"

        # Claude-only advisor: resolve attach, build options (old SDKs may drop
        # extra_args), then steer only when the advisor flag actually landed.
        advisor_extra = self._resolved_advisor_extra_args()

        async def can_use_tool(tool_name: str, tool_input: dict[str, Any], *args: Any, **kwargs: Any):
            decision = self.permission_decision(tool_name, tool_input or {})
            if decision.allowed:
                return self._allow(sdk, tool_input or {})
            self.denials.append((tool_name, decision.reason))
            logger.info("claude-sdk gate denied %s: %s", tool_name, decision.reason)
            return self._deny(sdk, decision.reason)

        options = self._build_options(sdk, can_use_tool, advisor_extra=advisor_extra)
        if self._options_have_advisor(options, advisor_extra):
            from devcouncil.executors.advisor_tool import (
                advisor_steering_text,
                advisor_user_cost_trim,
                warn_advisor_preflight,
            )

            for warning in warn_advisor_preflight(
                env=self.env or None,
                main_model=self.model,
                advisor_model=self.advisor_model,
            ):
                logger.warning("%s", warning)
            nudge = advisor_steering_text(repair=bool(prefix))
            cost_trim = advisor_user_cost_trim()
            prompt = f"{nudge}\n\n{cost_trim}\n\n{prompt}"
        try:
            return await self._run_async_once(sdk, prompt, options, task)
        except Exception as exc:  # a mid-run SDK failure is a failed execution, not a crash
            logger.exception(
                "claude-sdk executor raised mid-run for %s: %s",
                getattr(task, "id", "?"),
                exc,
            )
            reason = transient_error_in_text(str(exc))
            if reason is not None:
                import time

                logger.warning(
                    "claude-sdk transient failure for %s (%s); retrying once in 5s",
                    getattr(task, "id", "?"),
                    reason,
                )
                time.sleep(5.0)
                try:
                    return await self._run_async_once(sdk, prompt, options, task)
                except Exception as retry_exc:
                    logger.exception(
                        "claude-sdk retry failed for %s: %s",
                        getattr(task, "id", "?"),
                        retry_exc,
                    )
                    return ExecutionResult(
                        success=False,
                        message=f"Claude Agent SDK run failed after retry: {retry_exc}",
                    )
            return ExecutionResult(success=False, message=f"Claude Agent SDK run failed: {exc}")

    async def _run_async_once(self, sdk, prompt: str, options, task: Task) -> ExecutionResult:
        result_text: Optional[str] = None
        is_error = False
        async for message in sdk.query(prompt=prompt, options=options):
            session_id = self._message_session_id(message)
            if session_id:
                self.last_agent_session_id = session_id
            text, error = self._message_result(message)
            if text is not None:
                result_text = text
            if error:
                is_error = True
        if self.denials:
            denied = "; ".join(f"{name} ({reason})" for name, reason in self.denials)
            suffix = f" Gate denied {len(self.denials)} out-of-scope call(s): {denied}."
        else:
            suffix = ""
        message = (result_text or "Claude Agent SDK run finished.") + suffix
        return ExecutionResult(success=not is_error, message=message)

    def _resolved_advisor_extra_args(self) -> dict[str, str] | None:
        """Build SDK ``extra_args={"advisor": ...}`` when pairing looks safe."""
        if not self.advisor_model:
            return None
        from devcouncil.executors.advisor_tool import decide_advisor_attach

        decision, resolved, reason = decide_advisor_attach(
            main_model=self.model,
            advisor_model=self.advisor_model,
            env=self.env,
        )
        if decision != "attach" or not resolved:
            if reason:
                logger.warning(
                    "Skipping SDK advisor %s: %s",
                    self.advisor_model,
                    reason,
                )
            return None
        return {"advisor": resolved}

    @staticmethod
    def _options_have_advisor(options: Any, advisor_extra: dict[str, str] | None) -> bool:
        """True when advisor was requested and survived into the SDK options object."""
        if not advisor_extra:
            return False
        if isinstance(options, dict):
            extra = options.get("extra_args") or {}
        else:
            extra = getattr(options, "extra_args", None) or {}
        if not isinstance(extra, dict):
            return False
        return bool(extra.get("advisor"))

    def _build_options(self, sdk, can_use_tool, *, advisor_extra: dict[str, str] | None = None):
        options_cls = getattr(sdk, "ClaudeAgentOptions", None)
        kwargs: dict[str, Any] = {
            "cwd": str(self.project_root),
            "permission_mode": self.permission_mode,
            "can_use_tool": can_use_tool,
        }
        if self.model:
            kwargs["model"] = self.model
        if self.env:
            kwargs["env"] = dict(self.env)
        if advisor_extra:
            kwargs["extra_args"] = dict(advisor_extra)
        if options_cls is None:
            return kwargs
        # Only pass kwargs the SDK's options object actually accepts, so a version skew in
        # field names degrades gracefully instead of raising TypeError.
        try:
            return options_cls(**kwargs)
        except TypeError:
            # Progressively narrower kwarg sets: an SDK too old for ``env`` / ``extra_args``
            # still gets cwd/permission_mode/model rather than losing all options.
            obj = None
            dropped_extra = bool(advisor_extra)
            for keys in (
                {"cwd", "permission_mode", "model", "env", "extra_args"},
                {"cwd", "permission_mode", "model", "env"},
                {"cwd", "permission_mode", "model"},
            ):
                safe = {k: v for k, v in kwargs.items() if k in keys}
                try:
                    obj = options_cls(**safe)
                    if dropped_extra and "extra_args" not in safe:
                        logger.warning(
                            "claude-agent-sdk ClaudeAgentOptions rejected extra_args; "
                            "advisor_model=%r will not be applied. Upgrade claude-agent-sdk.",
                            self.advisor_model,
                        )
                    break
                except TypeError:
                    continue
            if obj is None:
                obj = options_cls()
                if dropped_extra:
                    logger.warning(
                        "claude-agent-sdk ClaudeAgentOptions could not accept advisor "
                        "extra_args; advisor_model=%r dropped.",
                        self.advisor_model,
                    )
            if hasattr(obj, "can_use_tool"):
                obj.can_use_tool = can_use_tool
            return obj

    @staticmethod
    def _message_session_id(message: Any) -> str | None:
        for getter in (lambda: getattr(message, "session_id", None),
                       lambda: message.get("session_id") if isinstance(message, dict) else None,
                       lambda: (getattr(message, "data", {}) or {}).get("session_id")):
            try:
                value = getter()
            except Exception:
                value = None
            if value:
                return str(value)
        return None

    @staticmethod
    def _message_result(message: Any) -> tuple[str | None, bool]:
        """Extract final result text and error flag from a terminal result message."""
        mtype = getattr(message, "type", None)
        if mtype is None and isinstance(message, dict):
            mtype = message.get("type")
        subtype = getattr(message, "subtype", None)
        if subtype is None and isinstance(message, dict):
            subtype = message.get("subtype")
        is_result = mtype == "result" or type(message).__name__ == "ResultMessage"
        if not is_result:
            return None, False
        result = getattr(message, "result", None)
        if result is None and isinstance(message, dict):
            result = message.get("result")
        is_error = getattr(message, "is_error", None)
        if is_error is None and isinstance(message, dict):
            is_error = message.get("is_error")
        if is_error is None:
            is_error = subtype not in (None, "success")
        return (str(result) if result is not None else None), bool(is_error)
