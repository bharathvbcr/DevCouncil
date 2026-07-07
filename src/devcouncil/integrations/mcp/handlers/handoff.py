"""Agent handoff MCP tool handler."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)


async def handle_handoff_agent(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    from_agent, arg_error = required_string_argument(arguments, "from_agent")
    if arg_error:
        return arg_error
    to_agent, arg_error = required_string_argument(arguments, "to_agent")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    cli_args = [
        "handoff-leased", task_id,
        "--lease-token", lease_token,
        "--from", from_agent or "",
        "--to", to_agent or "",
        "--json",
    ]
    instruction = str(arguments.get("instruction") or "")
    if instruction:
        cli_args.extend(["--instruction", instruction])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("ok"):
        from devcouncil.integrations.mcp.util import error_text
        return error_text(
            str(payload.get("error") or "handoff failed"),
            code=str(payload.get("code") or "handoff_failed"),
            task_id=task_id,
        )
    return json_text(payload)
