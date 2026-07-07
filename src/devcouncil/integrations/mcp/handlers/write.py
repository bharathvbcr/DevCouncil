"""Write and patch MCP tool handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)


async def handle_write_file(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    rel_path, arg_error = required_string_argument(arguments, "path")
    if arg_error:
        return arg_error
    content = arguments.get("content")
    if not isinstance(content, str):
        return error_text("content must be a string", code="invalid_arguments", argument="content")
    assert task_id is not None and lease_token is not None and rel_path is not None
    cli_args = [
        "write", task_id,
        "--lease-token", lease_token,
        "--path", rel_path,
        "--content", content,
        "--json",
    ]
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def handle_apply_patch(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    unified_diff = arguments.get("unified_diff")
    if not isinstance(unified_diff, str) or not unified_diff.strip():
        return error_text("unified_diff must be a non-empty string", code="invalid_arguments", argument="unified_diff")
    assert task_id is not None and lease_token is not None
    cli_args = [
        "apply-patch", task_id,
        "--lease-token", lease_token,
        "--unified-diff", unified_diff,
        "--json",
    ]
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
