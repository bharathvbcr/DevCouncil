"""Next-task selection MCP tool handler."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    optional_string_argument,
    parse_cli_json,
    run_cli_command,
)


async def handle_next_task(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    status_filter = optional_string_argument(arguments, "status")
    if status_filter == "":
        return error_text("status must be a string", code="invalid_arguments", argument="status")
    client_id = optional_string_argument(arguments, "client_id")
    if client_id == "":
        return error_text("client_id must be a string", code="invalid_arguments", argument="client_id")
    cli_args = ["next-task", "--json"]
    if status_filter:
        cli_args.extend(["--status", status_filter])
    if client_id:
        cli_args.extend(["--client-id", client_id])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
