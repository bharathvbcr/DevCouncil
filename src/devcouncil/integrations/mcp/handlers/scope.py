"""Task scope update MCP tool handler."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    optional_string_list_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)


async def handle_update_task_scope(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    expected_tests, arg_error = optional_string_list_argument(arguments, "expected_tests")
    if arg_error:
        return arg_error
    allowed_commands, arg_error = optional_string_list_argument(arguments, "allowed_commands")
    if arg_error:
        return arg_error
    cli_args = [
        "scope", "update", task_id,
        "--lease-token", lease_token,
        "--json",
    ]
    for test in expected_tests:
        cli_args.extend(["--expected-test", test])
    for cmd in allowed_commands:
        cli_args.extend(["--allowed-command", cmd])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
