"""Write and patch MCP tool handlers."""

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


async def handle_write_file(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    lease_token = optional_string_argument(arguments, "lease_token") or ""
    rel_path, arg_error = required_string_argument(arguments, "path")
    if arg_error:
        return arg_error
    content = arguments.get("content")
    if not isinstance(content, str):
        return error_text("content must be a string", code="invalid_arguments", argument="content")
    assert rel_path is not None
    cli_args = ["write"]
    if task_id:
        cli_args.append(task_id)
    cli_args.extend([
        "--lease-token", lease_token,
        "--path", rel_path,
        "--content", content,
        "--json",
    ])
    payload, cli_error = parse_cli_json(await asyncio.to_thread(run_cli_command, cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def handle_apply_patch(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    lease_token = optional_string_argument(arguments, "lease_token") or ""
    unified_diff = arguments.get("unified_diff")
    if not isinstance(unified_diff, str) or not unified_diff.strip():
        return error_text("unified_diff must be a non-empty string", code="invalid_arguments", argument="unified_diff")
    cli_args = ["apply-patch"]
    if task_id:
        cli_args.append(task_id)
    cli_args.extend([
        "--lease-token", lease_token,
        "--unified-diff", unified_diff,
        "--json",
    ])
    payload, cli_error = parse_cli_json(await asyncio.to_thread(run_cli_command, cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
