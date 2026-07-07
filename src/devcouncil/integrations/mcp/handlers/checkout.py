"""Task checkout MCP tool handler."""

from __future__ import annotations

from pathlib import Path
from typing import Callable

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    optional_string_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)


async def handle_checkout_task(
    root: Path,
    db: object,
    arguments: dict,
    *,
    load_router: Callable[[Path], object | None],
) -> list[TextContent]:
    del db, load_router  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    client_id, arg_error = required_string_argument(arguments, "client_id")
    if arg_error:
        return arg_error
    assert task_id is not None and client_id is not None
    agent = optional_string_argument(arguments, "agent")
    if agent == "":
        return error_text("agent must be a string", code="invalid_arguments", argument="agent")
    force_value = arguments.get("force", False)
    if not isinstance(force_value, bool):
        return error_text("force must be a boolean", code="invalid_arguments", argument="force")
    cli_args = ["checkout", task_id, "--client-id", client_id, "--json"]
    if agent:
        cli_args.extend(["--agent", agent])
    if force_value:
        cli_args.append("--force")
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
