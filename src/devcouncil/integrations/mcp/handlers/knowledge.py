"""Knowledge selection MCP tool handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    parse_cli_json,
    required_string_argument,
    run_cli_command,
)
from devcouncil.knowledge.knowledge_select import select_knowledge_payload


async def handle_select_knowledge(root: Path, arguments: dict) -> list[TextContent]:
    goal, arg_error = required_string_argument(arguments, "goal")
    if arg_error:
        return arg_error
    assert goal is not None
    payload, _cli_error = parse_cli_json(
        run_cli_command(["okf", "select", "--json", "--goal", goal], root, truncate=False),
    )
    if payload is not None:
        return json_text(payload)
    return json_text(select_knowledge_payload(root, goal))
