"""Registry-backed MCP tools for transactional code intelligence."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Awaitable, Callable

from mcp.types import TextContent, Tool

from devcouncil.codeintel.query import CodeIntelQueryEngine
from devcouncil.codeintel.service import canonical_project_root, get_codeintel_service
from devcouncil.codeintel.sync import get_sync_coordinator
from devcouncil.integrations.mcp.util import error_text, json_text, with_codeintel_freshness

Handler = Callable[[Path, dict], Awaitable[list[TextContent]]]


def _schema(properties: dict, required: list[str] | None = None) -> dict:
    schema: dict = {"type": "object", "properties": {
        "projectPath": {
            "type": "string",
            "description": "Explicit repository path; defaults to the MCP server project.",
        },
        **properties,
    }}
    if required:
        schema["required"] = required
    return schema


def tools() -> list[Tool]:
    return [
        Tool(
            name="devcouncil_code_explore",
            description="Unified code exploration: source, callers/callees, semantic hops, and blast radius.",
            inputSchema=_schema({
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
            }, ["query"]),
        ),
        Tool(
            name="devcouncil_code_search",
            description="FTS5 symbol, qualified-name, and path search over the committed generation.",
            inputSchema=_schema({
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50},
            }, ["query"]),
        ),
        Tool(
            name="devcouncil_code_path",
            description="Shortest call/import/framework path with confidence and provenance per hop.",
            inputSchema=_schema({
                "from": {"type": "string"},
                "to": {"type": "string"},
                "maxDepth": {"type": "integer", "minimum": 1, "maximum": 64, "default": 32},
            }, ["from", "to"]),
        ),
        Tool(
            name="devcouncil_code_impact",
            description="Inbound symbol blast radius for one or more paths/symbols.",
            inputSchema=_schema({
                "targets": {"type": "array", "items": {"type": "string"}},
                "maxDepth": {"type": "integer", "minimum": 1, "maximum": 8, "default": 3},
            }, ["targets"]),
        ),
        Tool(
            name="devcouncil_code_dead",
            description="Confidence-tiered dead-code candidates; never deletes code.",
            inputSchema=_schema({
                "minimumConfidence": {
                    "type": "string",
                    "enum": ["extracted", "inferred", "ambiguous"],
                    "default": "inferred",
                },
            }),
        ),
        Tool(
            name="devcouncil_code_affected_tests",
            description="Tests reachable through the inbound blast radius of paths/symbols.",
            inputSchema=_schema({
                "targets": {"type": "array", "items": {"type": "string"}},
                "maxDepth": {"type": "integer", "minimum": 1, "maximum": 8, "default": 3},
            }, ["targets"]),
        ),
        Tool(
            name="devcouncil_code_sync",
            description="Reconcile and commit pending source changes to the canonical index.",
            inputSchema=_schema({
                "paths": {"type": "array", "items": {"type": "string"}},
            }),
        ),
        Tool(
            name="devcouncil_code_status",
            description="Canonical generation, native watcher backend, pending files, and degraded state.",
            inputSchema=_schema({}),
        ),
    ]


def resolve_root(default_root: Path, arguments: dict) -> Path:
    explicit = arguments.get("projectPath")
    return canonical_project_root(Path(explicit) if isinstance(explicit, str) and explicit else default_root)


async def _explore(root: Path, arguments: dict) -> list[TextContent]:
    return json_text(CodeIntelQueryEngine(root).explore(str(arguments["query"]), limit=int(arguments.get("limit", 20))))


async def _search(root: Path, arguments: dict) -> list[TextContent]:
    return json_text(CodeIntelQueryEngine(root).search(str(arguments["query"]), limit=int(arguments.get("limit", 50))))


async def _path(root: Path, arguments: dict) -> list[TextContent]:
    return json_text(CodeIntelQueryEngine(root).path(
        str(arguments["from"]), str(arguments["to"]), max_depth=int(arguments.get("maxDepth", 32))
    ))


async def _impact(root: Path, arguments: dict) -> list[TextContent]:
    return json_text(CodeIntelQueryEngine(root).impact(
        [str(value) for value in arguments.get("targets") or []],
        max_depth=int(arguments.get("maxDepth", 3)),
    ))


async def _dead(root: Path, arguments: dict) -> list[TextContent]:
    return json_text(CodeIntelQueryEngine(root).dead(
        minimum_confidence=str(arguments.get("minimumConfidence", "inferred"))
    ))


async def _affected(root: Path, arguments: dict) -> list[TextContent]:
    return json_text(CodeIntelQueryEngine(root).affected_tests(
        [str(value) for value in arguments.get("targets") or []],
        max_depth=int(arguments.get("maxDepth", 3)),
    ))


async def _sync(root: Path, arguments: dict) -> list[TextContent]:
    coordinator = get_sync_coordinator(root)
    supplied = [str(value) for value in arguments.get("paths") or []]
    changed = supplied or await asyncio.to_thread(coordinator.reconcile)
    ok = await asyncio.to_thread(coordinator.sync_now, changed)
    payload = {"ok": ok, "reconciled": changed, **coordinator.status().as_dict()}
    return json_text(payload) if ok else error_text(
        coordinator.status().last_error or coordinator.status().degraded_reason or "sync failed",
        code="codeintel_sync_failed",
        **payload,
    )


async def _status(root: Path, _arguments: dict) -> list[TextContent]:
    service = get_codeintel_service(root)
    return json_text({**service.status(), "sync": get_sync_coordinator(root).status().as_dict()})


REGISTRY: dict[str, Handler] = {
    "devcouncil_code_explore": _explore,
    "devcouncil_code_search": _search,
    "devcouncil_code_path": _path,
    "devcouncil_code_impact": _impact,
    "devcouncil_code_dead": _dead,
    "devcouncil_code_affected_tests": _affected,
    "devcouncil_code_sync": _sync,
    "devcouncil_code_status": _status,
}


async def dispatch(name: str, default_root: Path, arguments: dict) -> list[TextContent] | None:
    handler = REGISTRY.get(name)
    if handler is None:
        return None
    root = resolve_root(default_root, arguments)
    try:
        if name in {"devcouncil_code_sync", "devcouncil_code_status"}:
            return await handler(root, arguments)
        return await with_codeintel_freshness(root, lambda: handler(root, arguments))
    except FileNotFoundError as exc:
        return error_text(str(exc), code="codeintel_not_initialized", project_root=str(root))
    except (KeyError, TypeError, ValueError) as exc:
        return error_text(str(exc), code="invalid_arguments", tool=name)
