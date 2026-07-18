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
    "dev status *",
    "uv run dev status",
    "uv run dev status *",
    "dev tasks",
    "dev tasks *",
    "uv run dev tasks",
    "uv run dev tasks *",
    # Lease bootstrap: agents must acquire a task lease before any other shell.
    "dev approve",
    "dev approve *",
    "uv run dev approve",
    "uv run dev approve *",
    "dev checkout *",
    "uv run dev checkout *",
    "dev next-task",
    "dev next-task *",
    "uv run dev next-task",
    "uv run dev next-task *",
    "git status",
    "git diff",
    "git diff *",
    # Harmless status probes agents chain after allowlisted commands.
    "echo",
    "echo *",
    "true",
    ":",
)

# Lease lifecycle and repo maintenance commands are always allowed for the lease
# holder (or when no task is bound), independent of task-specific allowed_commands.
_LEASE_LIFECYCLE_ALLOWED_COMMANDS = (
    "dev release *",
    "uv run dev release *",
    "dev lease *",
    "uv run dev lease *",
    "dev scope *",
    "uv run dev scope *",
    "dev map",
    "dev map *",
    "uv run dev map",
    "uv run dev map *",
    "dev doctor",
    "dev doctor *",
    "uv run dev doctor",
    "uv run dev doctor *",
    # Orientation / graph inspection without a lease (MCP substitute path).
    "dev graph",
    "dev graph *",
    "uv run dev graph",
    "uv run dev graph *",
    "dev run-cmd *",
    "uv run dev run-cmd *",
    "python -m pytest *",
    "uv run python -m pytest *",
    "uv run pytest *",
    "pytest *",
)

# Path-prefixed project CLIs (``.venv/bin/dev``, absolute hook paths) and ``uv run
# --project …`` wrappers must match the same allowlist entries as bare ``dev …``.
_UV_RUN_DIR_FLAG_RE = re.compile(
    r"^(?P<head>uv\s+run)(?:\s+(?:--project|--directory|-p)\s+\S+)+(?P<tail>\s+.+)$"
)
_DEV_BINARIES = frozenset({"dev", "dev.exe", "devcouncil", "devcouncil.exe"})
_CD_SEGMENT_RE = re.compile(r"^(?:cd|pushd|popd)(?:\s|$)")
# Trailing shell redirections break fnmatch against ``dev map *``; strip for matching only.
_REDIRECT_TAIL_RE = re.compile(
    r"(?:\s+(?:\d*)(?:>>|>|<|&>|&>>)\s*\S+|\s+\d*>&\d+)+\s*$"
)


def normalize_allowlist_command(command: str) -> str:
    """Collapse path-prefixed ``dev``/``devcouncil`` and ``uv run`` dir flags.

    Hooks install absolute ``.venv/bin/dev`` paths; agents often copy that form into
    Shell. Without normalization those commands miss ``dev map`` / ``dev status``
    allowlist entries and fail closed until checkout.

    Only rewrites bare ``dev``/``devcouncil`` tokens or executables under a ``bin`` /
    ``Scripts`` directory — never a repo folder whose basename happens to be
    ``DevCouncil``.
    """
    normalized = " ".join(command.split())
    if not normalized:
        return normalized
    normalized = _REDIRECT_TAIL_RE.sub("", normalized).rstrip()
    flagged = _UV_RUN_DIR_FLAG_RE.match(normalized)
    if flagged is not None:
        normalized = f"{flagged.group('head')}{flagged.group('tail')}"
    tokens = normalized.split()
    rewritten: list[str] = []
    for token in tokens:
        posix = token.replace("\\", "/")
        path = Path(posix)
        name = path.name.lower()
        parent = path.parent.name.lower()
        bare = "/" not in posix and name in _DEV_BINARIES
        under_bin = name in _DEV_BINARIES and parent in {"bin", "scripts"}
        if bare or under_bin:
            rewritten.append("dev")
        else:
            rewritten.append(token)
    return " ".join(rewritten)

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
        raw = " ".join(command.split())
        normalized = normalize_allowlist_command(raw)
        if not normalized:
            return PolicyDecision(
                action="deny",
                reason="Empty command is not allowed.",
                target=command,
                task_id=task.id if task else None,
            )

        # ``cd`` / ``pushd`` / ``popd`` alone cannot write; agents chain them before
        # allowlisted ``dev map`` (e.g. ``cd repo && .venv/bin/dev map``).
        if _CD_SEGMENT_RE.match(normalized):
            return PolicyDecision(
                action="allow",
                reason="Working-directory change is not gated.",
                target=normalized,
                task_id=task.id if task else None,
            )

        if any(
            fnmatch.fnmatch(normalized, allowed) for allowed in _LEASE_LIFECYCLE_ALLOWED_COMMANDS
        ):
            return PolicyDecision(
                action="allow",
                reason="Lease lifecycle or repo maintenance command allowed.",
                target=normalized,
                task_id=task.id if task else None,
            )

        if any(fnmatch.fnmatch(normalized, allowed) for allowed in _NO_TASK_ALLOWED_COMMANDS):
            return PolicyDecision(
                action="allow",
                reason="Bootstrap or read-only command allowed.",
                target=normalized,
                task_id=task.id if task else None,
            )

        if task is None:
            return PolicyDecision(
                action="deny",
                reason=(
                    "Shell commands require an active task lease. "
                    "Bootstrap with `dev checkout <TASK>`, or use allowlisted "
                    "orientation commands (`dev status`, `dev map`, `dev doctor`, "
                    "`dev graph …`)."
                ),
                target=normalized,
            )

        # Match allowlist entries against both raw and normalized forms so a task
        # that lists `.venv/bin/dev *` still works, while path-prefixed `dev map`
        # continues to hit lifecycle patterns after normalization.
        task_patterns = list(task.allowed_commands)
        if any(
            fnmatch.fnmatch(normalized, allowed) or fnmatch.fnmatch(raw, allowed)
            for allowed in task_patterns
        ):
            return PolicyDecision(
                action="allow",
                reason="Command matches task allowed_commands.",
                target=normalized,
                task_id=task.id,
            )
        if any(
            fnmatch.fnmatch(normalized, allowed) or fnmatch.fnmatch(raw, allowed)
            for allowed in self.global_allowed_commands
        ):
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
            neighbor_decision = self._neighbor_policy_decision(normalized, task)
            if neighbor_decision is not None:
                return neighbor_decision
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

    def _neighbor_policy_decision(self, path: str, task: Task) -> PolicyDecision | None:
        """Soft-block writes outside planned files unless same subsystem or a neighbor."""
        try:
            from devcouncil.indexing.subsystem_map import (
                area_for_path,
                are_neighbors,
            )
            from devcouncil.utils.json_persist import read_json

            map_path = self.project_root / ".devcouncil" / "repo_map.json"
            if not map_path.is_file():
                return None
            loaded = read_json(map_path)
            if not isinstance(loaded, dict):
                return None
            data = loaded
            target_area = area_for_path(path, data)
            if not target_area:
                return None
            planned_areas = {
                area_for_path(pf.path.replace("\\", "/"), data)
                for pf in task.planned_files
            }
            planned_areas.discard(None)
            if not planned_areas:
                return None
            if target_area in planned_areas:
                return PolicyDecision(
                    action="allow",
                    reason="File is in the same subsystem as a planned file.",
                    target=path,
                    task_id=task.id,
                )
            if any(
                are_neighbors(target_area, planned_area, data)
                for planned_area in planned_areas
            ):
                return PolicyDecision(
                    action="allow",
                    reason="File is in a neighboring subsystem of a planned file.",
                    target=path,
                    task_id=task.id,
                )
            return PolicyDecision(
                action="deny",
                reason=(
                    f"Task {task.id} does not authorize changes to {path} "
                    f"(subsystem `{target_area}` is outside planned files and not a "
                    f"declared neighbor). Expand scope with `dev scope update`."
                ),
                target=path,
                task_id=task.id,
            )
        except Exception:
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
