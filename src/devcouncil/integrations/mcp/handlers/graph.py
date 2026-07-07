"""Code-review graph context MCP tool handlers."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    run_cli_command,
)


async def handle_graph_context(root: Path, arguments: dict) -> list[TextContent]:
    files = arguments.get("files", [])
    if not isinstance(files, list):
        files = []
    file_args: list[str] = []
    for item in files:
        if isinstance(item, str) and item:
            file_args.extend(["--file", item])
    result = run_cli_command(["graph-context", "--json", *file_args], root)
    if result.get("ok"):
        try:
            payload = json.loads(str(result.get("stdout") or "{}"))
            return json_text(payload)
        except json.JSONDecodeError:
            pass
    from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter

    context = CodeReviewGraphAdapter(root).get_context(
        [file for file in files if isinstance(file, str)]
    )
    return [TextContent(type="text", text=context.model_dump_json(indent=2))]
