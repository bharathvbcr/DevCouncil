"""AST match and LSP status MCP tool handlers."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import TextContent

from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.indexing.lsp import LspInspector
from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    optional_string_argument,
)

_AST_MATCHER_CACHE: dict[str, AstMatcher] = {}
_LSP_INSPECTOR_CACHE: dict[str, LspInspector] = {}


def reset_caches() -> None:
    """Drop per-root AST/LSP caches (for test isolation)."""
    _AST_MATCHER_CACHE.clear()
    _LSP_INSPECTOR_CACHE.clear()


def get_ast_matcher(root: Path) -> AstMatcher:
    key = str(root.resolve())
    matcher = _AST_MATCHER_CACHE.get(key)
    if matcher is None:
        matcher = AstMatcher(root)
        _AST_MATCHER_CACHE[key] = matcher
    return matcher


def get_lsp_inspector(root: Path) -> LspInspector:
    key = str(root.resolve())
    inspector = _LSP_INSPECTOR_CACHE.get(key)
    if inspector is None:
        inspector = LspInspector(root)
        _LSP_INSPECTOR_CACHE[key] = inspector
    return inspector


async def handle_lsp_status(root: Path, arguments: dict) -> list[TextContent]:
    _ = arguments
    client_enabled = False
    try:
        from devcouncil.indexing.lsp_client import lsp_refs_enabled

        client_enabled = lsp_refs_enabled(root)
    except Exception:
        client_enabled = False
    return [
        TextContent(
            type="text",
            text=get_lsp_inspector(root).summary_json(client_enabled=client_enabled),
        )
    ]


async def handle_ast_match(root: Path, arguments: dict) -> list[TextContent]:
    query = optional_string_argument(arguments, "query")
    language = optional_string_argument(arguments, "language")
    kind = optional_string_argument(arguments, "kind")
    for arg_name, value in [("query", query), ("language", language), ("kind", kind)]:
        if value == "":
            return error_text(f"{arg_name} must be a string", code="invalid_arguments", argument=arg_name)
    limit = int_argument(arguments, "limit", 100, minimum=1, maximum=500)
    matches = get_ast_matcher(root).match(
        query=query or "",
        language=language,
        kind=kind,
        limit=limit,
    )
    return [TextContent(type="text", text=json.dumps({"matches": [item.model_dump() for item in matches]}, indent=2))]
