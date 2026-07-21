"""Git diff MCP tool handlers."""

from __future__ import annotations

import asyncio
import subprocess
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    CLI_TIMEOUT_SECONDS,
    error_text,
    is_git_repo,
    json_text,
    optional_string_argument,
    optional_string_list_argument,
    truncate_text,
)
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import TaskRepository

_BINARY_PROBE_BYTES = 8192


def _run_git(root: Path, args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=CLI_TIMEOUT_SECONDS,
    )


def _parse_name_status_z(data: str) -> dict[str, str]:
    """Parse ``git diff --name-status -z`` into ``{path: status}``.

    Rename/copy records are ``STATUS\\0OLD\\0NEW\\0``; other records are
    ``STATUS\\0PATH\\0``. The new path is the authoritative key for renames.
    """
    status_by_path: dict[str, str] = {}
    parts = data.split("\0")
    i = 0
    while i < len(parts):
        status = parts[i]
        if not status:
            i += 1
            continue
        kind = status[0]
        if kind in "RC" and i + 2 < len(parts):
            new_path = parts[i + 2].replace("\\", "/")
            if new_path:
                status_by_path[new_path] = status
            i += 3
            continue
        if i + 1 < len(parts):
            path = parts[i + 1].replace("\\", "/")
            if path:
                status_by_path[path] = status
            i += 2
            continue
        break
    return status_by_path


def _format_untracked_file_diff(rel_path: str, full_path: Path) -> tuple[str, int]:
    """Return ``(unified_diff_fragment, addition_count)`` for a new untracked file."""
    try:
        raw = full_path.read_bytes()
    except OSError:
        return "", 0

    header = [
        f"diff --git a/{rel_path} b/{rel_path}",
        "new file mode 100644",
        "--- /dev/null",
        f"+++ b/{rel_path}",
    ]
    if b"\0" in raw[:_BINARY_PROBE_BYTES]:
        return "\n".join([*header, f"Binary files /dev/null and b/{rel_path} differ"]), 0

    text = raw.decode("utf-8", errors="replace")
    if not text:
        return "\n".join(header) + "\n", 0

    lines = text.splitlines()
    if text.endswith(("\n", "\r")):
        line_count = len(lines)
    else:
        line_count = max(len(lines), 1)

    diff_lines = [*header, f"@@ -0,0 +1,{line_count} @@"]
    diff_lines.extend(f"+{line}" for line in lines)
    return "\n".join(diff_lines), line_count


def _list_untracked(root: Path, paths: list[str]) -> tuple[list[str], str | None]:
    """List untracked paths; return ``(paths, error)`` when the Git call fails."""
    args = ["git", "ls-files", "--others", "--exclude-standard", "-z"]
    if paths:
        args.append("--")
        args.extend(paths)
    try:
        proc = _run_git(root, args)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return [], str(exc)
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or f"git ls-files exited {proc.returncode}").strip()
        return [], detail or f"git ls-files exited {proc.returncode}"
    return [p.replace("\\", "/") for p in proc.stdout.split("\0") if p.strip()], None


def _collect_untracked(
    root: Path,
    paths: list[str],
    *,
    known_paths: set[str],
) -> tuple[list[dict[str, object]], str, str | None]:
    """Build file entries and unified diff for untracked files in scope."""
    untracked, err = _list_untracked(root, paths)
    if err is not None:
        return [], "", err

    files: list[dict[str, object]] = []
    fragments: list[str] = []
    for rel in untracked:
        if rel in known_paths:
            continue
        full = root / rel
        if not full.is_file():
            continue
        fragment, additions = _format_untracked_file_diff(rel, full)
        if not fragment:
            continue
        files.append({
            "path": rel,
            "status": "A",
            "additions": additions,
            "deletions": 0,
        })
        fragments.append(fragment.rstrip("\n"))
    unified = "\n".join(fragments)
    if unified:
        unified += "\n"
    return files, unified, None


async def git_diff(root: Path, paths: list[str], staged: bool) -> dict[str, object]:
    """Compute a (optionally path-scoped, optionally staged) git diff."""
    diff_args = ["git", "diff"]
    numstat_args = ["git", "diff", "--numstat"]
    namestatus_args = ["git", "diff", "--name-status", "-z"]
    if staged:
        for args in (diff_args, numstat_args, namestatus_args):
            args.append("--cached")
    if paths:
        for args in (diff_args, numstat_args, namestatus_args):
            args.append("--")
            args.extend(paths)

    def _run(args: list[str]) -> subprocess.CompletedProcess[str]:
        return _run_git(root, args)

    try:
        loop = asyncio.get_event_loop()
        diff_proc, numstat_proc, namestatus_proc = await asyncio.gather(
            loop.run_in_executor(None, _run, diff_args),
            loop.run_in_executor(None, _run, numstat_args),
            loop.run_in_executor(None, _run, namestatus_args),
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"ok": False, "files": [], "unified_diff": "", "truncated": False, "error": str(exc)}

    for proc, label in (
        (diff_proc, "git diff"),
        (numstat_proc, "git diff --numstat"),
        (namestatus_proc, "git diff --name-status"),
    ):
        if proc.returncode != 0:
            detail = (proc.stderr or proc.stdout or f"{label} exited {proc.returncode}").strip()
            return {
                "ok": False,
                "files": [],
                "unified_diff": "",
                "truncated": False,
                "error": detail or f"{label} exited {proc.returncode}",
                "staged": staged,
            }

    status_by_path = _parse_name_status_z(namestatus_proc.stdout)

    files: list[dict[str, object]] = []
    for line in numstat_proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        added_str, deleted_str, file_path = parts[0], parts[1], parts[-1]
        # Rename numstat without -z may use "old => new"; prefer the new side.
        if " => " in file_path:
            file_path = file_path.split(" => ", 1)[-1]
        file_path = file_path.replace("\\", "/")
        files.append({
            "path": file_path,
            "status": status_by_path.get(file_path, "M"),
            "additions": int(added_str) if added_str.isdigit() else 0,
            "deletions": int(deleted_str) if deleted_str.isdigit() else 0,
        })

    unified_parts = [diff_proc.stdout.rstrip("\n")] if diff_proc.stdout else []
    if not staged:
        known = {str(entry["path"]) for entry in files}
        untracked_files, untracked_diff, untracked_err = await asyncio.get_event_loop().run_in_executor(
            None,
            lambda: _collect_untracked(root, paths, known_paths=known),
        )
        if untracked_err is not None:
            return {
                "ok": False,
                "files": [],
                "unified_diff": "",
                "truncated": False,
                "error": untracked_err,
                "staged": staged,
            }
        files.extend(untracked_files)
        if untracked_diff:
            unified_parts.append(untracked_diff.rstrip("\n"))

    combined = "\n".join(part for part in unified_parts if part)
    if combined:
        combined += "\n"
    unified_diff, truncated = truncate_text(combined)
    return {"ok": True, "files": files, "unified_diff": unified_diff, "truncated": truncated, "staged": staged}


def _empty_diff_payload(staged: bool) -> dict[str, object]:
    return {"ok": True, "files": [], "unified_diff": "", "truncated": False, "staged": staged}


async def handle_get_diff(root: Path, db: Database | None, arguments: dict) -> list[TextContent]:
    if not is_git_repo(root):
        return error_text("get_diff requires a git repository.", code="not_a_git_repo")
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    explicit_paths, arg_error = optional_string_list_argument(arguments, "paths")
    if arg_error:
        return arg_error
    staged_value = arguments.get("staged", False)
    if not isinstance(staged_value, bool):
        return error_text("staged must be a boolean", code="invalid_arguments", argument="staged")

    scope_paths: list[str] = [p.replace("\\", "/") for p in explicit_paths]
    task_scoped = False
    if task_id:
        if db is None:
            return error_text(
                "DevCouncil not initialized in this directory.",
                code="not_initialized",
            )
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
        if task is None:
            return error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
        planned = [pf.path.replace("\\", "/") for pf in task.planned_files]
        planned_set = set(planned)
        task_scoped = True
        if explicit_paths:
            # Intersect only — explicit paths must never broaden task scope.
            scope_paths = [p for p in scope_paths if p in planned_set]
        else:
            scope_paths = planned

    if task_scoped and not scope_paths:
        # Empty planned scope or empty intersection: fail closed to empty, never full repo.
        return json_text(_empty_diff_payload(staged_value))

    return json_text(await git_diff(root, scope_paths, staged_value))
