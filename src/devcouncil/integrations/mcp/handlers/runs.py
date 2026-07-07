"""Agent run listing and inspection MCP tool handlers."""

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
    truncate_text,
)


def _cli_json(root: Path, args: list[str]) -> tuple[dict | None, list[TextContent] | None]:
    result = run_cli_command(args, root)
    stdout = str(result.get("stdout") or "").strip()
    if stdout:
        try:
            return json.loads(stdout), None
        except json.JSONDecodeError:
            pass
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "CLI command failed")
        return None, error_text(stderr, code="cli_failed")
    return None, error_text("CLI command returned invalid JSON", code="cli_parse_error")


async def handle_list_agent_runs(root: Path, arguments: dict) -> list[TextContent]:
    status_filter = optional_string_argument(arguments, "status")
    if status_filter == "":
        return error_text("status must be a string", code="invalid_arguments", argument="status")
    limit = int_argument(arguments, "limit", 20, minimum=1, maximum=500)
    cli_args = ["runs", "list", "--json", "--limit", str(limit)]
    if status_filter:
        cli_args.extend(["--status", status_filter])
    payload, cli_error = _cli_json(root, cli_args)
    if cli_error:
        return cli_error
    assert payload is not None
    runs = payload.get("runs") or []
    if not isinstance(runs, list):
        runs = []
    total = payload.get("total", payload.get("count", len(runs)))
    return json_text({
        "ok": True,
        "runs": runs,
        "total": total,
        "returned": len(runs),
    })


async def handle_get_run(root: Path, arguments: dict) -> list[TextContent]:
    run_id, arg_error = required_string_argument(arguments, "run_id")
    if arg_error:
        return arg_error
    assert run_id is not None
    payload, cli_error = _cli_json(root, ["runs", "show", run_id, "--json"])
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("ok", True):
        return error_text(
            str(payload.get("error") or f"Run {run_id} not found."),
            code=str(payload.get("code") or "not_found"),
            run_id=run_id,
        )
    tail = str(payload.get("transcript_tail") or "")
    truncated_tail, truncated = truncate_text(tail)
    return json_text({
        "ok": True,
        "run_id": payload.get("run_id", run_id),
        "manifest": payload.get("manifest"),
        "orphaned": payload.get("orphaned", False),
        "transcript_path": payload.get("transcript_path"),
        "transcript_tail": truncated_tail,
        "transcript_truncated": truncated,
    })
