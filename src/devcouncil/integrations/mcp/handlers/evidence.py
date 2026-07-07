"""Evidence read/write MCP tool handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    json_text,
    optional_string_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)


async def handle_append_evidence(root: Path, db: object, arguments: dict) -> list[TextContent]:
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
    summary_text, arg_error = required_string_argument(arguments, "summary")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    exit_code = arguments.get("exit_code", 0)
    if not isinstance(exit_code, int) or isinstance(exit_code, bool):
        return error_text("exit_code must be an integer", code="invalid_arguments")
    cli_args = [
        "evidence-append", task_id,
        "--lease-token", lease_token,
        "--command", command or "",
        "--summary", summary_text or "",
        "--exit-code", str(exit_code),
        "--json",
    ]
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def handle_get_evidence(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    command_filter = optional_string_argument(arguments, "command")
    if command_filter == "":
        return error_text("command must be a string", code="invalid_arguments", argument="command")
    limit = int_argument(arguments, "limit", 20, minimum=1, maximum=100)
    cli_args = ["evidence-list", task_id, "--limit", str(limit), "--json"]
    if command_filter:
        cli_args.extend(["--command", command_filter])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
