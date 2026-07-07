"""Task lease MCP tool handlers (release, renew, list)."""

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


async def handle_release_task(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    payload, cli_error = parse_cli_json(
        run_cli_command(["release", task_id, "--lease-token", lease_token, "--json"], root),
    )
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def handle_renew_lease(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    lease_token, arg_error = required_string_argument(arguments, "lease_token")
    if arg_error:
        return arg_error
    assert task_id is not None and lease_token is not None
    ttl_value = arguments.get("ttl_seconds")
    if ttl_value is not None and (not isinstance(ttl_value, int) or isinstance(ttl_value, bool)):
        return error_text("ttl_seconds must be an integer", code="invalid_arguments", argument="ttl_seconds")
    cli_args = ["lease", "renew", task_id, "--lease-token", lease_token, "--json"]
    if isinstance(ttl_value, int) and not isinstance(ttl_value, bool):
        cli_args.extend(["--ttl-seconds", str(ttl_value)])
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def handle_list_leases(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    active_only = arguments.get("active_only", True)
    if not isinstance(active_only, bool):
        return error_text("active_only must be a boolean", code="invalid_arguments", argument="active_only")
    cli_args = ["lease", "list", "--json"]
    if not active_only:
        cli_args.append("--all")
    payload, cli_error = parse_cli_json(run_cli_command(cli_args, root))
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)
