import fnmatch
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional

from devcouncil.domain.task import Task
from devcouncil.execution.policy_engine import (
    PROTECTED_WRITE_PATTERNS,
    SECRET_PATH_PATTERNS,
    TaskPolicyEngine,
    normalize_repo_path,
)
from devcouncil.utils.redaction import SECRET_PATTERNS

# Splits a shell command into the segments a shell would execute independently, on
# the chaining/pipe/sequence operators. We deliberately do NOT try to parse quoting
# perfectly — any operator we miss only makes us evaluate a *larger* segment as a
# single command (which then fails the allowlist), so this errs toward DENY.
_SEGMENT_SPLIT_RE = re.compile(r"\s*(?:\|\||&&|\||;|\n)\s*")

# Strips a `bash -c "..."` / `sh -c '...'` wrapper so the inner command is what gets
# checked against the allowlist, closing the trivial obfuscation where a denied
# command is smuggled inside a shell wrapper.
_SHELL_WRAPPER_RE = re.compile(
    r"""^\s*(?:[A-Za-z0-9_./\\-]*?(?:bash|sh|zsh|dash|ksh))(?:\.exe)?\s+-[A-Za-z]*c\s+(?P<quote>['"])(?P<inner>.*)(?P=quote)\s*$"""
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
        self.policy_engine = TaskPolicyEngine(
            self.project_root,
            global_allowed_commands=self._load_global_allowed_commands(),
        )

    def _load_global_allowed_commands(self) -> list[str]:
        """Best-effort load of repo-wide allowed commands from config.

        Never raises — a missing/invalid config must not disable the gate, and the
        gate stays fail-closed (empty allowlist) when config can't be read."""
        try:
            from devcouncil.app.config import load_config

            execution = load_config(self.project_root).execution
            configured = getattr(execution, "global_allowed_commands", None)
            if isinstance(configured, (list, tuple)):
                return [str(item) for item in configured]
        except Exception:
            pass
        return []

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
            return self.evaluate_command(command, active_task)

        if tool_name in self.write_tools:
            target = self._extract_path(arguments)
            content = self._extract_content(arguments)
            return self.evaluate_file_write(target, active_task, content=content)

        return HookDecision("allow", "Tool is outside DevCouncil hook policy.")

    def evaluate_command(self, command: str, active_task: Optional[Task] = None) -> HookDecision:
        """Evaluate a shell command before execution.

        Two gates, deny wins:
          1. Git-safety regexes (force push, --no-verify, protected-branch resets).
          2. The task command allowlist: every segment of a chained command must be
             authorized by the active task (or the global allowlist). With no active
             task only the read-only no-task allowlist applies, so a Bash-routed
             write/destructive command can no longer escape the planned-files gate.
        This fails closed: anything we cannot positively authorize is denied."""
        if self.policy_engine is None:
            return HookDecision("deny", "No project root configured.", command)

        # 1) Git-safety check first — a hard deny here wins regardless of allowlist.
        git_decision = self.policy_engine.evaluate_hook_command(command)
        if git_decision.action == "deny":
            return HookDecision(git_decision.action, git_decision.reason, git_decision.target)

        # 2) Allowlist enforcement over every executed segment.
        segments = self._split_command_segments(command)
        if not segments:
            return HookDecision("deny", "Empty command is not allowed.", command)

        warn: HookDecision | None = None
        for segment in segments:
            decision = self.policy_engine.evaluate_command(segment, active_task)
            if decision.action == "deny":
                return HookDecision("deny", decision.reason, decision.target)
            if decision.action == "warn" and warn is None:
                warn = HookDecision("warn", decision.reason, decision.target)

        # A git-safety warn (e.g. direct push to a protected branch) should surface even
        # when every segment is otherwise allowed.
        if git_decision.action == "warn":
            return HookDecision(git_decision.action, git_decision.reason, git_decision.target)
        if warn is not None:
            return warn
        return HookDecision("allow", "Command authorized by task allowlist.", command)

    def _split_command_segments(self, command: str) -> list[str]:
        """Split a shell command on ; && || | and newlines, unwrapping bash -c wrappers.

        Each returned segment is itself unwrapped/re-split so a denied command nested
        inside a `bash -c "..."` wrapper is still checked. Errs toward DENY: anything
        ambiguous collapses into a larger segment that the allowlist will reject."""
        normalized = command.strip()
        if not normalized:
            return []

        wrapper = _SHELL_WRAPPER_RE.match(normalized)
        if wrapper is not None:
            # Recurse into the wrapped command so its own chaining is evaluated.
            return self._split_command_segments(wrapper.group("inner"))

        segments: list[str] = []
        for raw in _SEGMENT_SPLIT_RE.split(normalized):
            piece = raw.strip()
            if not piece:
                continue
            nested = _SHELL_WRAPPER_RE.match(piece)
            if nested is not None:
                segments.extend(self._split_command_segments(nested.group("inner")))
            else:
                segments.append(piece)
        return segments

    def evaluate_file_write(
        self,
        raw_path: Optional[str],
        active_task: Optional[Task],
        *,
        content: Optional[str] = None,
    ) -> HookDecision:
        if not raw_path:
            return HookDecision("allow", "No file path detected.")
        if self.policy_engine is None:
            return HookDecision("deny", "No project root configured.", raw_path)

        # Pre-action secret scan: refuse to write content that contains a secret,
        # before it ever lands on disk. Reuses the shared secret regexes.
        if content is not None:
            secret = self._scan_content_for_secret(content)
            if secret is not None:
                return HookDecision(
                    "deny",
                    f"Refusing to write content containing a potential {secret}.",
                    raw_path,
                )

        # Delegate to the engine, which normalizes via the shared normalize_repo_path and
        # denies out-of-root targets — so the path that is checked is the path enforced.
        decision = self.policy_engine.evaluate_file_change(raw_path, active_task)
        return HookDecision(decision.action, decision.reason, decision.target)

    def _scan_content_for_secret(self, content: str) -> Optional[str]:
        if not isinstance(content, str) or not content:
            return None
        for key_type, pattern in SECRET_PATTERNS.items():
            if pattern.search(content):
                return key_type
        return None

    def _extract_command(self, arguments: dict[str, Any]) -> str:
        value = arguments.get("command") or arguments.get("cmd") or arguments.get("script") or ""
        return str(value)

    def _extract_content(self, arguments: dict[str, Any]) -> Optional[str]:
        """Pull the to-be-written content from a write tool's arguments.

        Covers the common shapes across coding CLIs (content/new_str/new_string/text)."""
        for key in ("content", "new_str", "new_string", "text", "file_text", "contents"):
            value = arguments.get(key)
            if isinstance(value, str) and value:
                return value
        return None

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
        # Shared normalizer — single source of truth with TaskPolicyEngine.
        return normalize_repo_path(self.project_root, raw_path)[0]

    def _is_planned_file(self, path: str, task: Task) -> bool:
        for planned in task.planned_files:
            planned_path = self._normalize_path(planned.path)
            if path == planned_path or fnmatch.fnmatch(path, planned_path):
                return True
        return False

    def _matches_any(self, path: str, patterns: tuple[str, ...]) -> bool:
        return any(fnmatch.fnmatch(path, pattern) for pattern in patterns)
