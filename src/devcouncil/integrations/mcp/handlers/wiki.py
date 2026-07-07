"""Codebase wiki MCP tool handlers."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    optional_string_argument,
    run_cli_command,
    truncate_text,
)
from devcouncil.knowledge.wiki_read import read_wiki_page


def _apply_body_truncation(payload: dict) -> dict:
    if "body" not in payload:
        return payload
    body, truncated = truncate_text(str(payload.get("body") or ""))
    updated = dict(payload)
    updated["body"] = body
    updated["truncated"] = truncated or bool(payload.get("truncated"))
    return updated


async def handle_wiki_page(root: Path, arguments: dict) -> list[TextContent]:
    page = optional_string_argument(arguments, "page")
    query = optional_string_argument(arguments, "query")
    result = run_cli_command(
        _wiki_cli_args(page=page, query=query),
        root,
    )
    if result.get("ok"):
        import json

        try:
            payload = json.loads(str(result.get("stdout") or "{}"))
        except json.JSONDecodeError:
            payload = read_wiki_page(root, page=page, query=query)
    else:
        payload = read_wiki_page(root, page=page, query=query)
    if not payload.get("ok", True):
        return error_text(
            str(payload.get("error") or "Wiki read failed"),
            code=str(payload.get("code") or "error"),
            **{k: v for k, v in payload.items() if k not in {"ok", "error", "code"}},
        )
    return json_text(_apply_body_truncation(payload))


def _wiki_cli_args(*, page: str | None, query: str | None) -> list[str]:
    args = ["wiki", "read", "--json"]
    if page:
        args.extend(["--page", page])
    if query:
        args.extend(["--query", query])
    return args
