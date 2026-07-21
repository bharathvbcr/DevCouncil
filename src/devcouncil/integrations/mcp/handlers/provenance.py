"""Task provenance and MCP corpus resource handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import Resource, TextContent
from pydantic import AnyUrl

from devcouncil.integrations.mcp.util import (
    json_text,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
    run_cli_json,
)
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


async def handle_get_task_provenance(root: Path, db: object, arguments: dict) -> list[TextContent]:
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    payload, cli_error = run_cli_json(["provenance", task_id, "--json"], root)
    if cli_error:
        return cli_error
    assert payload is not None
    return json_text(payload)


async def list_resources(root: Path) -> list[Resource]:
    """Expose the DevCouncil corpus as browsable MCP resources."""
    payload, _cli_error = parse_cli_json(
        run_cli_command(["resource", "list", "--json"], root, truncate=False),
    )
    if payload is not None:
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
    result = run_cli_command(["resource", "read", key], root, truncate=False)
    stdout = result.get("stdout")
    if result.get("ok") and stdout is not None:
        return str(stdout)
    from devcouncil.reporting.mcp_resources import read_mcp_resource

    return read_mcp_resource(root, key)
