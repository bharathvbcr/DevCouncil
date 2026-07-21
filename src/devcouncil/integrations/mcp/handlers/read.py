"""Read-file MCP tool handler."""

from __future__ import annotations

import hashlib
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    is_secret_path,
    json_text,
    optional_string_argument,
    required_string_argument,
    truncate_text,
    within_root,
)
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import TaskRepository


async def handle_read_file(
    root: Path,
    arguments: dict,
    db: Database | None = None,
) -> list[TextContent]:
    rel_path, arg_error = required_string_argument(arguments, "path")
    if arg_error:
        return arg_error
    assert rel_path is not None
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
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
        planned = {pf.path.replace("\\", "/") for pf in task.planned_files}
        normalized = rel_path.replace("\\", "/")
        if normalized not in planned:
            # Fail closed: never broaden task scope to arbitrary repo paths.
            return error_text(
                f"Path {normalized} is outside task {task_id} planned-file scope.",
                code="out_of_scope",
                path=normalized,
                task_id=task_id,
            )
    if is_secret_path(root, rel_path):
        return error_text(
            "Refusing to read a secret/credential path.",
            code="secret_path", path=rel_path,
        )
    target = within_root(root, rel_path)
    if target is None:
        return error_text("path escapes the project root", code="path_escape", path=rel_path)
    if not target.exists() or not target.is_file():
        return error_text(f"File not found: {rel_path}", code="not_found", path=rel_path)
    try:
        raw = target.read_bytes()
    except OSError as exc:
        return error_text(f"Read failed: {exc}", code="read_failed", path=rel_path)
    sha256 = hashlib.sha256(raw).hexdigest()
    text = raw.decode("utf-8", errors="replace")
    all_lines = text.splitlines()
    line_count = len(all_lines)
    line_range = optional_string_argument(arguments, "line_range")
    if line_range == "":
        return error_text("line_range must be a string", code="invalid_arguments", argument="line_range")
    selected = all_lines
    if line_range:
        try:
            start_str, _, end_str = line_range.partition("-")
            start = max(1, int(start_str))
            end = int(end_str) if end_str else line_count
        except ValueError:
            return error_text("line_range must look like '10-40'", code="invalid_arguments", argument="line_range")
        selected = all_lines[start - 1:end]
    else:
        offset = int_argument(arguments, "offset", 0, minimum=0, maximum=10_000_000)
        limit_value = arguments.get("limit")
        if isinstance(limit_value, int) and not isinstance(limit_value, bool):
            limit = max(1, limit_value)
            selected = all_lines[offset:offset + limit]
        elif offset:
            selected = all_lines[offset:]
    windowed = "\n".join(selected)
    content, truncated = truncate_text(windowed)
    return json_text({
        "ok": True,
        "path": rel_path.replace("\\", "/"),
        "content": content,
        "sha256": sha256,
        "line_count": line_count,
        "truncated": truncated,
    })
