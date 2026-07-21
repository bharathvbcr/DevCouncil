"""Code-review graph context MCP tool handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    json_text,
    parse_cli_json,
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
    payload, _cli_error = parse_cli_json(
        run_cli_command(["graph-context", "--json", *file_args], root, truncate=False),
    )
    if payload is not None:
        return json_text(payload)
    from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter

    context = CodeReviewGraphAdapter(root).get_context(
        [file for file in files if isinstance(file, str)]
    )
    return [TextContent(type="text", text=context.model_dump_json(indent=2))]
