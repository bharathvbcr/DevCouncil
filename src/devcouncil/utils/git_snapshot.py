"""Batch git worktree queries into a minimal set of subprocess calls.

Verifier hot paths previously invoked ``git diff``, ``git diff --name-only``, and
``git ls-files --others`` independently (and sometimes repeatedly within one
``verify_task``).  ``GitWorktreeSnapshot.capture`` runs at most three plumbing
commands — ``rev-parse``, ``diff HEAD`` (when HEAD exists), and
``status --porcelain -z`` — and caches the parsed results for the rest of the
verification pass.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Union

from devcouncil.utils.proc import GIT_TIMEOUT, git_output, run_git

PathLike = Union[str, Path]


@dataclass
class GitWorktreeSnapshot:
    """Cached worktree state from a single batched git probe."""

    has_head: bool
    tracked_diff: str = ""
    tracked_changed_files: List[str] = field(default_factory=list)
    untracked_files: List[str] = field(default_factory=list)
    cached_diff: str = ""
    working_diff: str = ""
    status_paths: List[str] = field(default_factory=list)

    @classmethod
    def capture(cls, root: PathLike, *, timeout: float = GIT_TIMEOUT) -> GitWorktreeSnapshot:
        root = Path(root)
        has_head = (
            run_git(["rev-parse", "--verify", "HEAD"], root, timeout=timeout).returncode == 0
        )
        status_entries = _parse_status(root, timeout=timeout)
        untracked = [e.path for e in status_entries if e.xy == "??"]
        status_paths = [e.path for e in status_entries]

        if has_head:
            tracked_diff = git_output(["diff", "HEAD"], root, timeout=timeout, default="")
            tracked_changed = sorted({e.path for e in status_entries if e.xy != "??"})
            if not tracked_changed and tracked_diff.strip():
                name_out = git_output(
                    ["diff", "HEAD", "--name-only"], root, timeout=timeout, default=""
                )
                tracked_changed = [p.replace("\\", "/") for p in name_out.splitlines() if p.strip()]
            return cls(
                has_head=True,
                tracked_diff=tracked_diff,
                tracked_changed_files=tracked_changed,
                untracked_files=sorted(set(untracked)),
                status_paths=status_paths,
            )

        cached_diff = git_output(["diff", "--cached"], root, timeout=timeout, default="")
        working_diff = git_output(["diff"], root, timeout=timeout, default="")
        if not untracked:
            untracked = _ls_untracked(root, timeout=timeout)
        if not status_paths:
            status_paths = sorted(set(untracked))
        return cls(
            has_head=False,
            cached_diff=cached_diff,
            working_diff=working_diff,
            untracked_files=sorted(set(untracked)),
            status_paths=status_paths,
        )


@dataclass(frozen=True)
class _StatusEntry:
    xy: str
    path: str


def _parse_status(root: Path, *, timeout: float) -> List[_StatusEntry]:
    result = run_git(["status", "--porcelain", "-z", "-u"], root, timeout=timeout)
    if result.returncode != 0 or not result.stdout:
        return []
    entries: List[_StatusEntry] = []
    parts = result.stdout.split("\0")
    i = 0
    while i < len(parts):
        entry = parts[i]
        if not entry or len(entry) < 3:
            i += 1
            continue
        xy = entry[:2]
        rest = entry[3:].replace("\\", "/")
        if "R" in xy:
            if i + 1 < len(parts) and parts[i + 1]:
                entries.append(_StatusEntry(xy=xy, path=parts[i + 1].replace("\\", "/")))
                i += 2
                continue
        entries.append(_StatusEntry(xy=xy, path=rest))
        i += 1
    return entries


def _ls_untracked(root: Path, *, timeout: float) -> List[str]:
    out = git_output(
        ["ls-files", "--others", "--exclude-standard"],
        root,
        timeout=timeout,
        default="",
    )
    return [p.replace("\\", "/") for p in out.splitlines() if p.strip()]
