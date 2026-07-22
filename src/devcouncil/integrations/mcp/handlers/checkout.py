"""Task checkout MCP tool handler."""

from __future__ import annotations

from pathlib import Path
from typing import Callable

from mcp.types import TextContent

from devcouncil.integrations.mcp.handlers.task import _applied_skill_names
from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    optional_string_argument,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
    run_cli_json,
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
    if isinstance(payload, dict) and "applied_skills" not in payload:
        task: dict = {}
        show_payload, _show_error = run_cli_json(["show", task_id, "--json"], root)
        if isinstance(show_payload, dict) and isinstance(show_payload.get("task"), dict):
            task = show_payload["task"]
        payload = {**payload, "applied_skills": _applied_skill_names(root, task)}
    return json_text(payload)
