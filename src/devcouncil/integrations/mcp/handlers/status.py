"""Status, gaps, and next-actions MCP tool handlers."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    json_text,
    optional_string_argument,
    required_string_argument,
    run_cli_command,
)


def _cli_json(root: Path, args: list[str]) -> tuple[dict | None, list[TextContent] | None]:
    result = run_cli_command(args, root)
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "CLI command failed")
        return None, error_text(stderr, code="cli_failed")
    try:
        return json.loads(str(result.get("stdout") or "{}")), None
    except json.JSONDecodeError:
        return None, error_text("CLI command returned invalid JSON", code="cli_parse_error")


async def handle_status(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    result = run_cli_command(["status", "--json"], root)
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "status command failed")
        return error_text(stderr, code="cli_failed")
    try:
        payload = json.loads(str(result.get("stdout") or "{}"))
    except json.JSONDecodeError:
        return error_text("status command returned invalid JSON", code="cli_parse_error")
    if not payload.get("initialized"):
        return error_text("DevCouncil not initialized in this directory.", code="not_initialized")
    summary = payload.get("coverage_summary") or {}
    phase = payload.get("phase", "UNKNOWN")
    status_str = (
        f"Phase: {phase}\n"
        f"Requirements: {summary.get('total_requirements', 0)} ({summary.get('requirements_without_tasks', 0)} unmapped)\n"
        f"Tasks: {summary.get('total_tasks', 0)} ({summary.get('tasks_without_requirements', 0)} orphaned)\n"
        f"Gaps: {summary.get('total_gaps', 0)} ({summary.get('blocking_gaps', 0)} blocking)\n"
    )
    return [TextContent(type="text", text=status_str)]


async def handle_report(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    result = run_cli_command(["report"], root)
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "report command failed")
        return error_text(stderr, code="cli_failed")
    return [TextContent(type="text", text=str(result.get("stdout") or ""))]


async def handle_get_gaps(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    blocking_only = bool(arguments.get("blocking_only", False))
    cli_args = ["gaps", "--json", "--task-id", task_id]
    if blocking_only:
        cli_args.append("--blocking-only")
    payload, cli_error = _cli_json(root, cli_args)
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("initialized", True):
        return error_text("DevCouncil not initialized in this directory.", code="not_initialized")
    return json_text(payload)


async def handle_get_next_actions(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    payload, cli_error = _cli_json(root, ["gaps", "--json", "--task-id", task_id, "--next-actions"])
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("ok", True):
        return error_text(
            str(payload.get("error") or "next-actions command failed"),
            code="cli_failed",
        )
    return json_text(payload)


async def handle_list_tasks(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    status_filter = optional_string_argument(arguments, "status")
    if status_filter == "":
        return error_text("status must be a string", code="invalid_arguments", argument="status")
    limit = int_argument(arguments, "limit", 100, minimum=1, maximum=500)
    offset = int_argument(arguments, "offset", 0, minimum=0, maximum=1_000_000)
    cli_args = ["tasks", "--json", "--limit", str(limit), "--offset", str(offset)]
    if status_filter:
        cli_args.extend(["--status", status_filter])
    payload, cli_error = _cli_json(root, cli_args)
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
