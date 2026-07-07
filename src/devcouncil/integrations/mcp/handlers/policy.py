"""Policy and command-recording MCP tool handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    optional_string_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)

RECORD_COMMAND_STATUSES = frozenset({"started", "finished", "failed", "blocked"})


async def handle_policy_check_write(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    path, arg_error = required_string_argument(arguments, "path")
    if arg_error:
        return arg_error
    assert path is not None
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    cli_args = ["policy-check", path, "--json"]
    if task_id:
        cli_args.extend(["--task-id", task_id])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def handle_record_command(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    command, arg_error = required_string_argument(arguments, "command")
    if arg_error:
        return arg_error
    status, arg_error = required_string_argument(arguments, "status")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    if status not in RECORD_COMMAND_STATUSES:
        return error_text(
            f"status must be one of {sorted(RECORD_COMMAND_STATUSES)}",
            code="invalid_arguments",
            argument="status",
        )
    exit_code = arguments.get("exit_code")
    if exit_code is not None and (not isinstance(exit_code, int) or isinstance(exit_code, bool)):
        return error_text("exit_code must be an integer", code="invalid_arguments")
    cli_args = [
        "record-command", task_id,
        "--lease-token", lease_token,
        "--command", command or "",
        "--status", status or "finished",
        "--json",
    ]
    if isinstance(exit_code, int):
        cli_args.extend(["--exit-code", str(exit_code)])
    reason = str(arguments.get("reason") or "")
    if reason:
        cli_args.extend(["--reason", reason])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
