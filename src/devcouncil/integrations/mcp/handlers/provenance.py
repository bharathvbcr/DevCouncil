"""Task provenance and MCP corpus resource handlers."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import Resource, TextContent
from pydantic import AnyUrl

from devcouncil.integrations.mcp.util import json_text, required_string_argument, run_cli_command
from devcouncil.knowledge.resource_discovery import (
    discover_knowledge_sources,
    knowledge_source_uri,
)

__all__ = [
    "discover_knowledge_sources",
    "handle_get_task_provenance",
    "knowledge_settings",
    "knowledge_source_uri",
    "list_resources",
    "read_resource",
]

# Re-export for callers that imported from this module.
from devcouncil.knowledge.resource_discovery import knowledge_settings  # noqa: E402


def _cli_json(root: Path, args: list[str]) -> tuple[dict | None, list[TextContent] | None]:
    result = run_cli_command(args, root)
    stdout = str(result.get("stdout") or "").strip()
    if stdout:
        try:
            return json.loads(stdout), None
        except json.JSONDecodeError:
            pass
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "CLI command failed")
        from devcouncil.integrations.mcp.util import error_text
        return None, error_text(stderr, code="cli_failed")
    from devcouncil.integrations.mcp.util import error_text
    return None, error_text("CLI command returned invalid JSON", code="cli_parse_error")


async def handle_get_task_provenance(root: Path, db: object, arguments: dict) -> list[TextContent]:
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    payload, cli_error = _cli_json(root, ["provenance", task_id, "--json"])
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def list_resources(root: Path) -> list[Resource]:
    """Expose the DevCouncil corpus as browsable MCP resources."""
    result = run_cli_command(["resource", "list", "--json"], root)
    stdout = str(result.get("stdout") or "").strip()
    if stdout:
        try:
            payload = json.loads(stdout)
            descriptors = payload.get("resources") or []
            if isinstance(descriptors, list):
                return [
                    Resource(
                        uri=AnyUrl(item["uri"]),
                        name=item["name"],
                        description=item["description"],
                        mimeType=item["mimeType"],
                    )
                    for item in descriptors
                    if isinstance(item, dict) and item.get("uri")
                ]
        except json.JSONDecodeError:
            pass
    # Fallback when CLI is unavailable (e.g. during early init).
    from devcouncil.reporting.mcp_resources import list_mcp_resource_uris

    return [
        Resource(
            uri=AnyUrl(item["uri"]),
            name=item["name"],
            description=item["description"],
            mimeType=item["mimeType"],
        )
        for item in list_mcp_resource_uris(root)
    ]


async def read_resource(root: Path, uri: AnyUrl) -> str:
    key = str(uri).rstrip("/")
    result = run_cli_command(["resource", "read", key], root)
    stdout = result.get("stdout")
    if result.get("ok") and stdout is not None:
        return str(stdout)
    from devcouncil.reporting.mcp_resources import read_mcp_resource

    return read_mcp_resource(root, key)
