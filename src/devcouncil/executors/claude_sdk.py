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
from typing import Any, List, Optional

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.execution.executor import ExecutionResult, Executor
from devcouncil.execution.hook_policy import HookDecision, HookPolicy
from devcouncil.execution.prompt_builder import PromptBuilder

logger = logging.getLogger(__name__)


class ClaudeSdkExecutor(Executor):
    """Run a DevCouncil task through the Claude Agent SDK with lease-aware gating."""

    def __init__(
        self,
        project_root: Path | str,
        *,
        active_task: Task | None = None,
        model: str | None = None,
        permission_mode: str = "default",
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
        self.permission_mode = permission_mode
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
        import asyncio

        try:
            return asyncio.run(self._run_async(task, requirements))
        except RuntimeError as exc:
            # Missing SDK or a nested event loop — report cleanly, never crash the caller.
            logger.error("claude-sdk executor could not run %s: %s", getattr(task, "id", "?"), exc)
            return ExecutionResult(success=False, message=str(exc))

    async def _run_async(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        sdk = self._load_sdk()
        prompt = PromptBuilder(self.project_root).build_task_prompt(task, requirements)
        # Parity with the subprocess executor: a repair run must carry the correction
        # manifest so the model knows what the prior attempt got wrong.
        from devcouncil.planning.correction_manifest import load_latest_correction_manifest

        correction = load_latest_correction_manifest(self.project_root, task.id)
        if correction is not None:
            prompt = (
                f"# DevCouncil Correction Manifest\n\n"
                f"{correction.model_dump_json(indent=2)}\n\n"
                f"{prompt}"
            )

        async def can_use_tool(tool_name: str, tool_input: dict[str, Any], *args: Any, **kwargs: Any):
            decision = self.permission_decision(tool_name, tool_input or {})
            if decision.allowed:
                return self._allow(sdk, tool_input or {})
            self.denials.append((tool_name, decision.reason))
            logger.info("claude-sdk gate denied %s: %s", tool_name, decision.reason)
            return self._deny(sdk, decision.reason)

        options = self._build_options(sdk, can_use_tool)
        result_text: Optional[str] = None
        is_error = False
        try:
            async for message in sdk.query(prompt=prompt, options=options):
                session_id = self._message_session_id(message)
                if session_id:
                    self.last_agent_session_id = session_id
                text, error = self._message_result(message)
                if text is not None:
                    result_text = text
                if error:
                    is_error = True
        except Exception as exc:  # a mid-run SDK failure is a failed execution, not a crash
            logger.exception("claude-sdk executor raised for %s", getattr(task, "id", "?"))
            return ExecutionResult(success=False, message=f"Claude Agent SDK run failed: {exc}")

        if self.denials:
            denied = "; ".join(f"{name} ({reason})" for name, reason in self.denials)
            suffix = f" Gate denied {len(self.denials)} out-of-scope call(s): {denied}."
        else:
            suffix = ""
        message = (result_text or "Claude Agent SDK run finished.") + suffix
        return ExecutionResult(success=not is_error, message=message)

    def _build_options(self, sdk, can_use_tool):
        options_cls = getattr(sdk, "ClaudeAgentOptions", None)
        kwargs: dict[str, Any] = {
            "cwd": str(self.project_root),
            "permission_mode": self.permission_mode,
            "can_use_tool": can_use_tool,
        }
        if self.model:
            kwargs["model"] = self.model
        if options_cls is None:
            return kwargs
        # Only pass kwargs the SDK's options object actually accepts, so a version skew in
        # field names degrades gracefully instead of raising TypeError.
        try:
            return options_cls(**kwargs)
        except TypeError:
            safe = {k: v for k, v in kwargs.items() if k in {"cwd", "permission_mode", "model"}}
            try:
                obj = options_cls(**safe)
            except TypeError:
                obj = options_cls()
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
