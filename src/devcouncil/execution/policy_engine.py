"""Shared task policy engine for shell commands and file changes."""

from __future__ import annotations

import fnmatch
import re
from pathlib import Path
from typing import Literal

from pydantic import BaseModel

from devcouncil.domain.task import PlannedFile, Task

# Precompiled git-safety patterns for hook-command evaluation (compiled once at import
# instead of on every evaluate_hook_command call).
_HARD_RESET_PROTECTED_RE = re.compile(r"\bgit\s+reset\s+--hard\s+(origin/)?(main|master)\b")
_FORCE_PUSH_FLAG_RE = re.compile(r"\bgit\s+push\b.*(\s--force(?:-with-lease)?\b|\s-f\b)")
_FORCE_PUSH_PLUS_REFSPEC_RE = re.compile(r"\bgit\s+push\s+\S+\s+\+\S")
_PROTECTED_BRANCH_PUSH_RE = re.compile(
    r"\bgit\s+push\s+\S+\s+((head:)?(main|master)|(main|master):\S+)\b"
)


class PolicyDecision(BaseModel):
    action: Literal["allow", "warn", "deny"]
    reason: str
    target: str
    task_id: str | None = None


def normalize_repo_path(project_root: Path, raw_path: str) -> tuple[str, bool]:
    """Resolve ``raw_path`` against ``project_root`` and report containment.

    Returns ``(normalized_posix_path, is_outside_root)``. This is the single source of
    truth used by both the task policy engine and the coding-CLI hook policy, so a path
    is normalized identically wherever it is checked — closing the bypass where a path
    that failed to resolve was returned raw and enforced differently than it was
    checked. ``is_outside_root`` is True for anything that escapes the project (absolute
    elsewhere, ``..`` traversal) or cannot be resolved, so callers fail closed."""
    cleaned = raw_path.strip().strip('"').replace("\\", "/")
    root = project_root.resolve()
    try:
        candidate = Path(cleaned)
        resolved = candidate.resolve() if candidate.is_absolute() else (root / cleaned).resolve()
    except OSError:
        fallback = cleaned[2:] if cleaned.startswith("./") else cleaned
        return fallback, True
    try:
        return resolved.relative_to(root).as_posix(), False
    except ValueError:
        return resolved.as_posix(), True


_NO_TASK_ALLOWED_COMMANDS = (
    "dev status",
    "dev tasks",
    "git status",
    "git diff",
    "git diff *",
)

# Shared with HookPolicy — keep these as the single source of truth for
# protected/secret path patterns.
PROTECTED_WRITE_PATTERNS = (
    "package.json",
    "pyproject.toml",
    "uv.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Dockerfile",
    "docker-compose.yml",
    ".github/workflows/*.yml",
    ".github/workflows/*.yaml",
    "schema.prisma",
    "wrangler.toml",
    "index.html",
)

SECRET_PATH_PATTERNS = (
    ".env",
    ".env.*",
    "**/.env",
    "**/.env.*",
    "**/credentials/**",
    "**/secrets/**",
    "**/*.pem",
    "**/*.key",
    # Private SSH keys and well-known credential/token files. These are never
    # writable through the gate so an agent cannot plant or overwrite credentials.
    "**/id_rsa",
    "**/id_dsa",
    "**/id_ecdsa",
    "**/id_ed25519",
    "id_rsa",
    "id_ed25519",
    ".npmrc",
    "**/.npmrc",
    ".pypirc",
    "**/.pypirc",
    ".netrc",
    "**/.netrc",
    "**/.aws/credentials",
    ".aws/credentials",
    "**/*.pfx",
    "**/*.p12",
    "*.pfx",
    "*.p12",
    ".git-credentials",
    "**/.git-credentials",
    "**/kube/config",
    "**/.kube/config",
)

_RESTRICTED_PATH_PATTERNS = (
    ".git/*",
    ".devcouncil/*",
    # Protect the client hook/agent configs themselves: an agent must not be able to
    # disarm or rewire the pre-action gate by editing its own client integration files.
    ".claude/*",
    ".claude/**",
    ".codex/*",
    ".codex/**",
    ".cursor/*",
    ".cursor/**",
    ".gemini/*",
    ".gemini/**",
    ".opencode/*",
    ".opencode/**",
    ".agents/*",
    ".agents/**",
    "opencode.json",
)


class TaskPolicyEngine:
    def __init__(
        self,
        project_root: Path,
        global_allowed_commands: list[str] | None = None,
    ):
        self.project_root = project_root.resolve()
        self.global_allowed_commands = global_allowed_commands or []

    def evaluate_command(self, command: str, task: Task | None) -> PolicyDecision:
        normalized = " ".join(command.split())
        if not normalized:
            return PolicyDecision(
                action="deny",
                reason="Empty command is not allowed.",
                target=command,
                task_id=task.id if task else None,
            )

        if task is None:
            if any(fnmatch.fnmatch(normalized, allowed) for allowed in _NO_TASK_ALLOWED_COMMANDS):
                return PolicyDecision(
                    action="allow",
                    reason="Read-only command allowed without active task.",
                    target=normalized,
                )
            return PolicyDecision(
                action="deny",
                reason="Shell commands require an active task lease.",
                target=normalized,
            )

        if any(fnmatch.fnmatch(normalized, allowed) for allowed in task.allowed_commands):
            return PolicyDecision(
                action="allow",
                reason="Command matches task allowed_commands.",
                target=normalized,
                task_id=task.id,
            )
        if any(fnmatch.fnmatch(normalized, allowed) for allowed in self.global_allowed_commands):
            return PolicyDecision(
                action="allow",
                reason="Command matches global allowed commands.",
                target=normalized,
                task_id=task.id,
            )
        return PolicyDecision(
            action="deny",
            reason="Command is not in task or global allowlists.",
            target=normalized,
            task_id=task.id,
        )

    def evaluate_file_change(
        self,
        path: str,
        task: Task | None,
        operation: Literal["create", "modify", "delete", "write"] = "write",
        *,
        internal: bool = False,
    ) -> PolicyDecision:
        normalized, outside_root = normalize_repo_path(self.project_root, path)
        task_id = task.id if task else None

        if outside_root:
            return PolicyDecision(
                action="deny",
                reason="Path is outside the project root.",
                target=normalized,
                task_id=task_id,
            )

        if self._matches_any(normalized, SECRET_PATH_PATTERNS):
            return PolicyDecision(
                action="deny",
                reason="Secret and credential paths are never writable.",
                target=normalized,
                task_id=task_id,
            )

        if not internal and self._matches_restricted(normalized):
            return PolicyDecision(
                action="deny",
                reason="Protected repository paths cannot be modified.",
                target=normalized,
                task_id=task_id,
            )

        if task is None:
            return PolicyDecision(
                action="deny",
                reason="No running DevCouncil task authorizes this file write.",
                target=normalized,
            )

        if self._matches_forbidden(normalized, task):
            return PolicyDecision(
                action="deny",
                reason="Path is listed in forbidden_changes.",
                target=normalized,
                task_id=task.id,
            )

        planned = self._planned_file_for(normalized, task)
        if planned is None:
            return PolicyDecision(
                action="deny",
                reason=f"Task {task.id} does not authorize changes to {normalized}.",
                target=normalized,
                task_id=task.id,
            )

        if planned.allowed_change == "read_only":
            return PolicyDecision(
                action="deny",
                reason="Planned file is read-only.",
                target=normalized,
                task_id=task.id,
            )
        if operation == "write":
            if planned.allowed_change not in {"create", "modify"}:
                return PolicyDecision(
                    action="deny",
                    reason=f"Operation {operation} not allowed for planned file.",
                    target=normalized,
                    task_id=task.id,
                )
        elif planned.allowed_change != operation:
            return PolicyDecision(
                action="deny",
                reason=f"Operation {operation} not allowed for planned file.",
                target=normalized,
                task_id=task.id,
            )

        if self._matches_any(normalized, PROTECTED_WRITE_PATTERNS):
            return PolicyDecision(
                action="warn",
                reason=f"{normalized} is a protected high-impact file; verification gates must approve it.",
                target=normalized,
                task_id=task.id,
            )

        return PolicyDecision(
            action="allow",
            reason="File change is allowed.",
            target=normalized,
            task_id=task.id,
        )

    def evaluate_hook_command(self, command: str) -> PolicyDecision:
        """Git safety checks for hook/shell tool paths."""
        normalized = " ".join(command.split())
        lowered = normalized.lower()
        if not normalized:
            return PolicyDecision(action="allow", reason="No command detected.", target=normalized)

        if "--no-verify" in lowered or "--no-gpg-sign" in lowered:
            return PolicyDecision(
                action="deny",
                reason="Verification bypass flags are not allowed.",
                target=normalized,
            )

        if _HARD_RESET_PROTECTED_RE.search(lowered):
            return PolicyDecision(
                action="deny",
                reason="Protected branch hard resets are not allowed.",
                target=normalized,
            )

        if _FORCE_PUSH_FLAG_RE.search(lowered) or _FORCE_PUSH_PLUS_REFSPEC_RE.search(lowered):
            # The second pattern catches the leading-plus refspec form
            # (`git push origin +HEAD:master`), which forces a non-fast-forward update
            # without the --force flag.
            return PolicyDecision(
                action="deny",
                reason="Force pushes are not allowed.",
                target=normalized,
            )

        if _PROTECTED_BRANCH_PUSH_RE.search(lowered):
            return PolicyDecision(
                action="warn",
                reason="Direct pushes to protected branches should go through verification gates.",
                target=normalized,
            )

        return PolicyDecision(action="allow", reason="Command is allowed.", target=normalized)

    def _normalize_path(self, raw_path: str) -> str:
        return normalize_repo_path(self.project_root, raw_path)[0]

    def _planned_file_for(self, path: str, task: Task) -> PlannedFile | None:
        for planned in task.planned_files:
            planned_path = planned.path.replace("\\", "/")
            if path == planned_path or fnmatch.fnmatch(path, planned_path):
                return planned
        return None

    def _matches_forbidden(self, path: str, task: Task) -> bool:
        for forbidden in task.forbidden_changes:
            pattern = forbidden.replace("\\", "/")
            if path == pattern or fnmatch.fnmatch(path, pattern):
                return True
        return False

    def _matches_restricted(self, path: str) -> bool:
        for pattern in _RESTRICTED_PATH_PATTERNS:
            if fnmatch.fnmatch(path, pattern) or path.startswith(pattern.strip("*")):
                return True
        return False

    def _matches_any(self, path: str, patterns: tuple[str, ...]) -> bool:
        return any(fnmatch.fnmatch(path, pattern) for pattern in patterns)
