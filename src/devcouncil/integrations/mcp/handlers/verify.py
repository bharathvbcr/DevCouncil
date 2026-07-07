"""Verify MCP tool handler."""

from __future__ import annotations

from pathlib import Path
from typing import Callable

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    optional_string_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)


async def handle_verify_task(
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
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    sandbox = optional_string_argument(arguments, "sandbox") or "local"
    cli_args = [
        "verify-leased", task_id,
        "--lease-token", lease_token,
        "--sandbox", sandbox,
        "--json",
    ]
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
