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


async def git_diff(root: Path, paths: list[str], staged: bool) -> dict[str, object]:
    """Compute a (optionally path-scoped, optionally staged) git diff."""
    diff_args = ["git", "diff"]
    numstat_args = ["git", "diff", "--numstat"]
    namestatus_args = ["git", "diff", "--name-status"]
    if staged:
        for args in (diff_args, numstat_args, namestatus_args):
            args.append("--cached")
    if paths:
        for args in (diff_args, numstat_args, namestatus_args):
            args.append("--")
            args.extend(paths)

    def _run(args: list[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            args, cwd=root, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=CLI_TIMEOUT_SECONDS,
        )

    try:
        loop = asyncio.get_event_loop()
        diff_proc, numstat_proc, namestatus_proc = await asyncio.gather(
            loop.run_in_executor(None, _run, diff_args),
            loop.run_in_executor(None, _run, numstat_args),
            loop.run_in_executor(None, _run, namestatus_args),
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"ok": False, "files": [], "unified_diff": "", "truncated": False, "error": str(exc)}

    status_by_path: dict[str, str] = {}
    for line in namestatus_proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2:
            status_by_path[parts[-1].replace("\\", "/")] = parts[0]

    files: list[dict[str, object]] = []
    for line in numstat_proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        added_str, deleted_str, file_path = parts[0], parts[1], parts[-1]
        file_path = file_path.replace("\\", "/")
        files.append({
            "path": file_path,
            "status": status_by_path.get(file_path, "M"),
            "additions": int(added_str) if added_str.isdigit() else 0,
            "deletions": int(deleted_str) if deleted_str.isdigit() else 0,
        })

    unified_diff, truncated = truncate_text(diff_proc.stdout)
    return {"ok": True, "files": files, "unified_diff": unified_diff, "truncated": truncated, "staged": staged}


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
    scope_paths: list[str] = list(explicit_paths)
    if task_id and db:
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
        if task is None:
            return error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
        for planned in task.planned_files:
            p = planned.path.replace("\\", "/")
            if p not in scope_paths:
                scope_paths.append(p)
    return json_text(await git_diff(root, scope_paths, staged_value))
