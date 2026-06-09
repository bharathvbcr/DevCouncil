import fnmatch
import re
from dataclasses import dataclass
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any, Optional

from devcouncil.domain.task import Task
from devcouncil.execution.policy_engine import (
    PROTECTED_WRITE_PATTERNS,
    SECRET_PATH_PATTERNS,
    TaskPolicyEngine,
)


@dataclass(frozen=True)
class HookDecision:
    action: str
    reason: str
    target: Optional[str] = None

    @property
    def allowed(self) -> bool:
        return self.action in {"allow", "warn"}


class HookPolicy:
    """Policy-backed hook checks for coding CLI tool-use events."""

    def __init__(self, project_root: Path | None = None):
        self.project_root = (project_root or Path(".")).resolve()
        self.policy_engine = TaskPolicyEngine(self.project_root)

    secret_path_patterns = SECRET_PATH_PATTERNS
    protected_path_patterns = PROTECTED_WRITE_PATTERNS
    write_tools = {
        "apply_patch",
        "edit",
        "edit_file",
        "replace",
        "write",
        "write_file",
        "Edit",
        "MultiEdit",
        "Write",
        "create_file",
        "str_replace",
        "search_replace",
    }
    shell_tools = {
        "bash",
        "exec",
        "exec_command",
        "local_shell",
        "run_command",
        "run_shell_command",
        "run_terminal_cmd",
        "shell",
        "shell_command",
        "Bash",
        "Shell",
    }

    def evaluate(self, call_data: dict[str, Any], active_task: Optional[Task]) -> HookDecision:
        tool_name = str(call_data.get("name") or call_data.get("tool_name") or call_data.get("tool") or "")
        arguments = call_data.get("arguments") or call_data.get("input") or call_data.get("tool_input") or {}
        if not isinstance(arguments, dict):
            arguments = {}

        if tool_name in self.shell_tools:
            command = self._extract_command(arguments)
            return self.evaluate_command(command)

        if tool_name in self.write_tools:
            target = self._extract_path(arguments)
            return self.evaluate_file_write(target, active_task)

        return HookDecision("allow", "Tool is outside DevCouncil hook policy.")

    def evaluate_command(self, command: str) -> HookDecision:
        if self.policy_engine is None:
            return HookDecision("allow", "No project root configured.")
        decision = self.policy_engine.evaluate_hook_command(command)
        return HookDecision(decision.action, decision.reason, decision.target)

    def evaluate_file_write(self, raw_path: Optional[str], active_task: Optional[Task]) -> HookDecision:
        if not raw_path:
            return HookDecision("allow", "No file path detected.")
        if self.policy_engine is None:
            return HookDecision("deny", "No project root configured.", raw_path)

        path = self._normalize_path(raw_path)
        decision = self.policy_engine.evaluate_file_change(path, active_task)
        return HookDecision(decision.action, decision.reason, decision.target)

    def _extract_command(self, arguments: dict[str, Any]) -> str:
        value = arguments.get("command") or arguments.get("cmd") or arguments.get("script") or ""
        return str(value)

    def _extract_path(self, arguments: dict[str, Any]) -> Optional[str]:
        value = (
            arguments.get("path")
            or arguments.get("file_path")
            or arguments.get("filepath")
            or arguments.get("filePath")
            or arguments.get("target")
            or arguments.get("target_file")
        )
        return str(value) if value else None

    def _normalize_path(self, raw_path: str) -> str:
        path = raw_path.strip().strip('"').replace("\\", "/")
        if self.project_root:
            try:
                candidate = Path(path)
                resolved = candidate.resolve() if candidate.is_absolute() else (self.project_root / path).resolve()
                return resolved.relative_to(self.project_root).as_posix()
            except (OSError, ValueError):
                pass
        if re.match(r"^[A-Za-z]:/", path):
            parts = PurePosixPath(path).parts
            path = "/".join(parts[1:])
        return path[2:] if path.startswith("./") else path

    def _is_planned_file(self, path: str, task: Task) -> bool:
        for planned in task.planned_files:
            planned_path = self._normalize_path(planned.path)
            if path == planned_path or fnmatch.fnmatch(path, planned_path):
                return True
        return False

    def _matches_any(self, path: str, patterns: tuple[str, ...]) -> bool:
        return any(fnmatch.fnmatch(path, pattern) for pattern in patterns)
