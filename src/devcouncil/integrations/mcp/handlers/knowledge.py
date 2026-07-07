"""Knowledge selection MCP tool handlers."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    required_string_argument,
    run_cli_command,
)
from devcouncil.knowledge.knowledge_select import select_knowledge_payload


async def handle_select_knowledge(root: Path, arguments: dict) -> list[TextContent]:
    goal, arg_error = required_string_argument(arguments, "goal")
    if arg_error:
        return arg_error
    assert goal is not None
    result = run_cli_command(["okf", "select", "--json", "--goal", goal], root)
    if result.get("ok"):
        try:
            payload = json.loads(str(result.get("stdout") or "{}"))
            return json_text(payload)
        except json.JSONDecodeError:
            pass
    return json_text(select_knowledge_payload(root, goal))
