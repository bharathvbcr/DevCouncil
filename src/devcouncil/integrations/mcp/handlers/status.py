"""Status, gaps, and next-actions MCP tool handlers."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    json_text,
    optional_string_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
    run_cli_json,
)

# Default MCP list/status projections target a ~32 KB agent context budget.
MCP_CONTEXT_BUDGET_CHARS = 32_000

_LIST_TASK_FIELDS = ("id", "title", "status", "priority", "requirement_ids", "lease")
_GAP_FIELDS = (
    "id",
    "severity",
    "gap_type",
    "blocking",
    "task_id",
    "requirement_id",
    "file",
    "line",
    "acceptance_criterion_id",
)
_ACTION_FIELDS = (
    "gap_id",
    "gap_type",
    "category",
    "severity",
    "blocking",
    "action",
    "file",
    "line",
    "suggested_command",
    "acceptance_criterion_id",
)


def _compact_task_row(task: object) -> dict[str, Any]:
    if not isinstance(task, dict):
        return {}
    row: dict[str, Any] = {key: task.get(key) for key in _LIST_TASK_FIELDS}
    row["requirements"] = task.get("requirement_ids") or []
    row.pop("requirement_ids", None)
    return row


def _compact_gap_row(gap: object) -> dict[str, Any]:
    if not isinstance(gap, dict):
        return {}
    return {key: gap.get(key) for key in _GAP_FIELDS}


def _compact_action_row(action: object) -> dict[str, Any]:
    if not isinstance(action, dict):
        return {}
    return {key: action.get(key) for key in _ACTION_FIELDS}


def _status_cli_error(cli_error: list[TextContent]) -> list[TextContent]:
    try:
        err = json.loads(cli_error[0].text)
    except (IndexError, json.JSONDecodeError, TypeError):
        return cli_error
    code = err.get("code")
    if code == "cli_parse_error":
        return error_text("status command returned invalid JSON", code="cli_parse_error")
    if code == "cli_failed":
        return error_text(str(err.get("error") or "status command failed"), code="cli_failed")
    return cli_error


async def handle_status(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db, arguments
    payload, cli_error = parse_cli_json(run_cli_command(["status", "--json"], root, truncate=False))
    if cli_error:
        return _status_cli_error(cli_error)
    assert payload is not None
    if not payload.get("initialized"):
        return error_text("DevCouncil not initialized in this directory.", code="not_initialized")
    summary = payload.get("coverage_summary") or {}
    live = payload.get("live_review") or {}
    phase = payload.get("phase", "UNKNOWN")
    lines = [
        f"Phase: {phase}",
        (
            f"Requirements: {summary.get('total_requirements', 0)} "
            f"({summary.get('requirements_without_tasks', 0)} unmapped)"
        ),
        (
            f"Tasks: {summary.get('total_tasks', 0)} "
            f"({summary.get('tasks_without_requirements', 0)} orphaned)"
        ),
        (
            f"Gaps: {summary.get('total_gaps', 0)} "
            f"({summary.get('blocking_gaps', 0)} blocking)"
        ),
        f"Live signals: {live.get('pending_signals', 0)}",
    ]
    return [TextContent(type="text", text="\n".join(lines) + "\n")]


async def handle_report(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db, arguments
    result = run_cli_command(["report"], root, truncate=True)
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "report command failed")
        return error_text(stderr, code="cli_failed")
    return [TextContent(type="text", text=str(result.get("stdout") or ""))]


async def handle_get_gaps(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    blocking_only = bool(arguments.get("blocking_only", False))
    cli_args = ["gaps", "--json", "--task-id", task_id]
    if blocking_only:
        cli_args.append("--blocking-only")
    payload, cli_error = run_cli_json(cli_args, root)
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("initialized", True):
        return error_text("DevCouncil not initialized in this directory.", code="not_initialized")
    gaps = payload.get("gaps")
    if isinstance(gaps, list):
        payload = {
            **payload,
            "gaps": [_compact_gap_row(gap) for gap in gaps],
            "gap_count": len(gaps),
        }
    return json_text(payload)


async def handle_get_next_actions(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    payload, cli_error = run_cli_json(["gaps", "--json", "--task-id", task_id, "--next-actions"], root)
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("ok", True):
        return error_text(
            str(payload.get("error") or "next-actions command failed"),
            code="cli_failed",
        )
    next_actions = payload.get("next_actions")
    advisory = payload.get("advisory_actions")
    updates: dict[str, Any] = {}
    if isinstance(next_actions, list):
        updates["next_actions"] = [_compact_action_row(a) for a in next_actions]
        updates["next_action_count"] = len(next_actions)
    if isinstance(advisory, list):
        updates["advisory_actions"] = [_compact_action_row(a) for a in advisory]
        updates["advisory_action_count"] = len(advisory)
    if updates:
        payload = {**payload, **updates}
    return json_text(payload)


async def handle_list_tasks(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db
    status_filter = optional_string_argument(arguments, "status")
    if status_filter == "":
        return error_text("status must be a string", code="invalid_arguments", argument="status")
    limit = int_argument(arguments, "limit", 100, minimum=1, maximum=500)
    offset = int_argument(arguments, "offset", 0, minimum=0, maximum=1_000_000)
    cli_args = ["tasks", "--json", "--limit", str(limit), "--offset", str(offset)]
    if status_filter:
        cli_args.extend(["--status", status_filter])
    payload, cli_error = run_cli_json(cli_args, root)
    if cli_error:
        return cli_error
    assert payload is not None
    tasks = payload.get("tasks")
    if isinstance(tasks, list):
        payload = {
            **payload,
            "tasks": [_compact_task_row(task) for task in tasks],
        }
    return json_text(payload)
