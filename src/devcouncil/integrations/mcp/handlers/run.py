"""Run-command MCP tool handler."""

from __future__ import annotations

import asyncio
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


async def handle_run_command(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    lease_token = optional_string_argument(arguments, "lease_token") or ""
    command, arg_error = required_string_argument(arguments, "command")
    if arg_error:
        return arg_error
    assert command is not None
    cli_args = ["run-cmd"]
    if task_id:
        cli_args.append(task_id)
    cli_args.extend([
        "--lease-token", lease_token,
        "--command", command,
        "--json",
    ])
    payload, cli_error = parse_cli_json(await asyncio.to_thread(run_cli_command, cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    if payload.get("code") == "command_not_allowed":
        return error_text(
            str(payload.get("error") or "Command is not in the task allowlist."),
            code="command_not_allowed",
            task_id=task_id,
            command=str(payload.get("command") or command),
        )
    if payload.get("code") == "run_failed":
        return error_text(
            str(payload.get("error") or "Could not run command"),
            code="run_failed",
            task_id=task_id,
        )
    return json_text(payload)
