"""Git diff fallbacks when no batched snapshot is available.

Extracted from ``verifier.py`` so the verifier orchestrator stays focused on gate
logic while pre-head / untracked / walk-the-tree paths live in one place.
"""

from __future__ import annotations

import fnmatch
import logging
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, List, Optional

from devcouncil.domain.checkpoint_refs import task_before_ref
from devcouncil.utils.proc import GIT_TIMEOUT

if TYPE_CHECKING:
    from devcouncil.utils.git_snapshot import GitWorktreeSnapshot

logger = logging.getLogger(__name__)

IGNORED_CHANGE_PATTERNS = (
    "__pycache__/*",
    "*/__pycache__/*",
    "*.pyc",
    "*.pyo",
    ".pytest_cache/*",
    ".mypy_cache/*",
    ".ruff_cache/*",
    ".devcouncil/*",
    ".gitignore",
)

MAX_UNTRACKED_DIFF_BYTES = 256_000


@dataclass
class GitDiffFallback:
    """Pre-snapshot git diff helpers used by :class:`~devcouncil.verification.verifier.Verifier`."""

    project_root: Path
    git_snapshot: Optional["GitWorktreeSnapshot"] = None
    untracked_cache: Optional[List[str]] = field(default=None, repr=False)

    def has_head(self) -> bool:
        if self.git_snapshot is not None:
            return self.git_snapshot.has_head
        try:
            return subprocess.run(
                ["git", "rev-parse", "--verify", "HEAD"],
                cwd=self.project_root,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=GIT_TIMEOUT,
            ).returncode == 0
        except (subprocess.TimeoutExpired, OSError) as exc:
            logger.warning("git rev-parse HEAD failed: %s", exc)
            return False

    def get_initial_repo_diff(self) -> str:
        parts: List[str] = []
        for cmd in (["git", "diff", "--cached"], ["git", "diff"]):
            result = subprocess.run(
                cmd,
                cwd=self.project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=GIT_TIMEOUT,
            )
            if result.returncode == 0 and result.stdout:
                parts.append(result.stdout)
        untracked_diff = self.get_untracked_files_diff()
        if untracked_diff:
            parts.append(untracked_diff)
        return "\n".join(parts)

    def get_status_files(self) -> List[str]:
        files: set[str] = set()
        commands = (
            ["git", "diff", "--cached", "--name-only"],
            ["git", "diff", "--name-only"],
            ["git", "ls-files", "--others", "--exclude-standard"],
        )
        for cmd in commands:
            try:
                output = subprocess.check_output(
                    cmd,
                    cwd=self.project_root,
                    stderr=subprocess.DEVNULL,
                    timeout=GIT_TIMEOUT,
                ).decode("utf-8", errors="replace").splitlines()
                files.update(path.replace("\\", "/") for path in output if path.strip())
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                continue
        if not files:
            files.update(self.walk_project_files())
        return self.filter_change_paths(sorted(files))

    def get_untracked_files(self) -> List[str]:
        if self.git_snapshot is not None:
            return self.filter_change_paths(self.git_snapshot.untracked_files)
        if self.untracked_cache is not None:
            return self.untracked_cache
        try:
            output = subprocess.check_output(
                ["git", "ls-files", "--others", "--exclude-standard"],
                cwd=self.project_root,
                stderr=subprocess.DEVNULL,
                timeout=GIT_TIMEOUT,
            ).decode("utf-8", errors="replace").splitlines()
            return self.filter_change_paths(output)
        except Exception as exc:
            logger.debug("Failed to list untracked files: %s", exc)
            return []

    def get_untracked_files_diff(self) -> str:
        parts: List[str] = []
        for rel_path in self.get_untracked_files():
            full_path = self.project_root / rel_path
            if not full_path.is_file():
                continue
            parts.append(self.format_new_file_diff(rel_path, full_path))
        return "\n".join(part for part in parts if part)

    def format_new_file_diff(self, rel_path: str, full_path: Path) -> str:
        try:
            raw = full_path.read_bytes()
        except Exception as exc:
            logger.debug("Failed to read untracked file %s: %s", rel_path, exc)
            return ""

        header = [
            f"diff --git a/{rel_path} b/{rel_path}",
            "new file mode 100644",
            "--- /dev/null",
            f"+++ b/{rel_path}",
        ]
        if b"\0" in raw[:8192]:
            return "\n".join([*header, f"Binary files /dev/null and b/{rel_path} differ"])

        truncated = len(raw) > MAX_UNTRACKED_DIFF_BYTES
        if truncated:
            raw = raw[:MAX_UNTRACKED_DIFF_BYTES]
        text = raw.decode("utf-8", errors="replace")
        lines = text.splitlines()
        if text.endswith(("\n", "\r")):
            line_count = len(lines)
        else:
            line_count = max(len(lines), 1 if text else 0)

        diff_lines = [*header, f"@@ -0,0 +1,{line_count} @@"]
        if not text:
            return "\n".join(header) + "\n"

        diff_lines.extend(f"+{line}" for line in lines)
        if truncated:
            diff_lines.append("+[devcouncil: untracked file diff truncated]")
        return "\n".join(diff_lines)

    def walk_project_files(self) -> List[str]:
        files: List[str] = []
        for path in self.project_root.rglob("*"):
            if not path.is_file():
                continue
            rel = path.relative_to(self.project_root).as_posix()
            if rel.startswith(".git/"):
                continue
            files.append(rel)
        return files

    def filter_change_paths(self, paths: List[str]) -> List[str]:
        return [
            path
            for path in (p.strip().replace("\\", "/") for p in paths)
            if path and not self.is_ignored_change(path)
        ]

    def is_ignored_change(self, path: str) -> bool:
        return any(fnmatch.fnmatch(path, pattern) for pattern in IGNORED_CHANGE_PATTERNS)

    def get_diff(self) -> str:
        snap = self.git_snapshot
        try:
            if snap is not None:
                if not snap.has_head:
                    parts = [snap.cached_diff, snap.working_diff]
                    untracked_diff = self.get_untracked_files_diff()
                    return "\n".join(part for part in [*parts, untracked_diff] if part)
                untracked_diff = self.get_untracked_files_diff()
                return "\n".join(part for part in [snap.tracked_diff, untracked_diff] if part)
            if not self.has_head():
                return self.get_initial_repo_diff()
            tracked_diff = subprocess.check_output(
                ["git", "diff", "HEAD"], cwd=self.project_root, timeout=GIT_TIMEOUT
            ).decode("utf-8", errors="replace")
            untracked_diff = self.get_untracked_files_diff()
            return "\n".join(part for part in [tracked_diff, untracked_diff] if part)
        except Exception as e:
            logger.warning("Failed to get git diff: %s", e)
            return ""

    def get_changed_files(self) -> List[str]:
        snap = self.git_snapshot
        try:
            if snap is not None:
                if not snap.has_head:
                    return self.filter_change_paths(snap.status_paths or snap.untracked_files)
                files = set(snap.tracked_changed_files)
                files.update(snap.untracked_files)
                return self.filter_change_paths(sorted(files))
            if not self.has_head():
                return self.get_status_files()
            output = subprocess.check_output(
                ["git", "diff", "HEAD", "--name-only"], cwd=self.project_root, timeout=GIT_TIMEOUT
            ).decode("utf-8", errors="replace").splitlines()
            files = set(output)
            files.update(self.get_untracked_files())
            return self.filter_change_paths(sorted(files))
        except Exception as e:
            logger.warning("Failed to get changed files: %s", e)
            return []

    def committed_task_diff(self, task_id: str) -> str:
        """Diff of work committed since the task's ``before`` checkpoint, or ""."""
        before_ref = task_before_ref(task_id)
        try:
            has_ref = subprocess.run(
                ["git", "rev-parse", "--verify", before_ref],
                cwd=self.project_root,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=GIT_TIMEOUT,
            ).returncode == 0
            if has_ref:
                return subprocess.check_output(
                    ["git", "diff", before_ref],
                    cwd=self.project_root,
                    stderr=subprocess.DEVNULL,
                    timeout=GIT_TIMEOUT,
                ).decode("utf-8", errors="replace")
        except Exception as e:
            logger.debug("Failed to read committed task diff for %s: %s", task_id, e)
        return ""
