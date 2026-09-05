"""Registry-backed MCP tools for transactional code intelligence.

Every tool here answers from the Rust ``devmap`` kernel through
``DevMapClient``, or reports that it could not. There is no second engine: a
Python fallback answering from a store the kernel does not write is the SC23
shape — two answers, no signal which one replied — and the last two holdouts,
``explore`` and ``affected_tests``, were migrated with it.

Those two were not merely slower. Since the Python writer was retired nothing
creates ``.devcouncil/codeintel/index.sqlite``, so ``CodeIntelQueryEngine``
raised ``FileNotFoundError`` on every call and both tools answered
``codeintel_not_initialized`` on a repository with a freshly built map.
"""

from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from typing import Any, Awaitable, Callable

from mcp.types import TextContent, Tool

from devcouncil.codeintel.service import canonical_project_root
from devcouncil.devmap_client import (
    BudgetedResponse,
    DevMapClient,
    DevMapClientError,
    resolution_unavailable_reason,
    try_connect,
    walk_incomplete_reason,
)
from devcouncil.integrations.mcp.util import error_text, json_text, with_codeintel_freshness

logger = logging.getLogger(__name__)

Handler = Callable[[Path, dict], Awaitable[list[TextContent]]]

#: Longest accepted value for a free-text argument (query, symbol, path).
#:
#: An unbounded string is a payload the server copies into a regex, a socket
#: frame and a log line before anything rejects it; the kernel's own transport
#: refuses a query above ``MAX_QUERY_BYTES`` (4 KiB), so accepting more here
#: only moves the refusal later and makes it less legible. Advertised in the
#: schema so a client is told the bound rather than discovering it as an error.
_MAX_STRING_LENGTH = 4096
#: Longest accepted path/target array. Every element costs at least one kernel
#: round trip, so an unbounded array is an unbounded amount of blocking work.
_MAX_ARRAY_ITEMS = 100


def _schema(properties: dict, required: list[str] | None = None) -> dict:
    schema: dict = {"type": "object", "properties": {
        "projectPath": {
            "type": "string",
            "maxLength": _MAX_STRING_LENGTH,
            "description": "Repository path inside the server root; defaults to that root.",
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
            input_schema=_schema({
                "query": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
            }, ["query"]),
        ),
        Tool(
            name="devcouncil_code_search",
            description="FTS5 symbol, qualified-name, and path search over the committed generation.",
            input_schema=_schema({
                "query": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50},
            }, ["query"]),
        ),
        Tool(
            name="devcouncil_code_path",
            description="Shortest call/import/framework path with confidence and provenance per hop.",
            input_schema=_schema({
                "from": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                "to": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                "maxDepth": {"type": "integer", "minimum": 1, "maximum": 64, "default": 32},
            }, ["from", "to"]),
        ),
        Tool(
            name="devcouncil_code_impact",
            description="Inbound symbol blast radius for one or more paths/symbols.",
            input_schema=_schema({
                "targets": {
                    "type": "array",
                    "items": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                    "maxItems": _MAX_ARRAY_ITEMS,
                },
                "maxDepth": {"type": "integer", "minimum": 1, "maximum": 8, "default": 3},
            }, ["targets"]),
        ),
        Tool(
            name="devcouncil_code_dead",
            description="Confidence-tiered dead-code candidates; never deletes code.",
            input_schema=_schema({
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
            input_schema=_schema({
                "targets": {
                    "type": "array",
                    "items": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                    "maxItems": _MAX_ARRAY_ITEMS,
                },
                "maxDepth": {"type": "integer", "minimum": 1, "maximum": 8, "default": 3},
            }, ["targets"]),
        ),
        Tool(
            name="devcouncil_code_sync",
            description="Reconcile and commit pending source changes to the canonical index.",
            input_schema=_schema({
                "paths": {
                    "type": "array",
                    "items": {"type": "string", "maxLength": _MAX_STRING_LENGTH},
                    "maxItems": _MAX_ARRAY_ITEMS,
                },
            }),
        ),
        Tool(
            name="devcouncil_code_status",
            description="Canonical generation, native watcher backend, pending files, and degraded state.",
            input_schema=_schema({}),
        ),
    ]


class ProjectPathOutsideRoot(ValueError):
    """A caller-supplied ``projectPath`` resolved outside the server's own root."""


def resolve_root(default_root: Path, arguments: dict) -> Path:
    """Resolve ``projectPath`` inside the server's root, or refuse it.

    ``canonical_project_root`` walks *upward* to the nearest ``.git`` /
    ``.devcouncil`` marker, so before this containment check any caller-supplied
    path selected any repository on the machine: ``{"projectPath": "/etc"}``
    resolved to ``/private/etc`` and ``{"projectPath": "~"}`` to the user's home.
    Sixteen tools took the argument and one of them, ``devcouncil_code_sync``,
    writes ``<root>/.devcouncil/repo_map.json`` — so an unconstrained path was a
    write outside the project the server was started for.

    ``DEVCOUNCIL_PROJECT_ROOT`` is the boundary, canonicalized exactly as the
    no-argument case already canonicalizes it, so every path that worked before
    and stayed inside the project still resolves to the same root. Both sides are
    fully resolved (symlinks included) before comparison, so neither a symlink
    inside the root pointing out nor one outside pointing in changes the verdict.
    A relative ``projectPath`` is taken against that boundary rather than the
    server process's working directory — the same rule ``within_root`` uses for
    every other path argument — because a relative path resolved against a CWD
    the caller cannot see is a different directory than the one it named.

    Raises:
        ProjectPathOutsideRoot: when the resolved path is not inside the root.
    """
    boundary = canonical_project_root(default_root)
    explicit = arguments.get("projectPath")
    if not isinstance(explicit, str) or not explicit:
        return boundary
    candidate = Path(explicit).expanduser()
    if not candidate.is_absolute():
        candidate = boundary / candidate
    resolved = canonical_project_root(candidate)
    try:
        resolved.relative_to(boundary)
    except ValueError:
        raise ProjectPathOutsideRoot(
            f"projectPath {explicit!r} resolves to {resolved}, which is outside "
            f"this server's project root {boundary}"
        ) from None
    return resolved


def _client_envelope(
    root: Path, client: DevMapClient, payload: dict[str, Any], *, operation: str = ""
) -> dict[str, Any]:
    """Wrap a Rust answer in the payload contract consumers already read.

    `operation` was missing here while the Python engine supplied it, and the
    gap was invisible for as long as a fallback existed: the handler-contract
    test passed because Python answered every call in an unbuilt repository.
    Removing the fallback made the Rust envelope the only answer and the missing
    key a KeyError — the fallback had been hiding a real contract gap, not
    covering for a transient one.
    """
    status = client.status()
    return {
        "ok": True,
        "operation": operation,
        "project_root": str(root.resolve()),
        "generation": status.generation_id or None,
        "schema_version": status.raw.get("schema_version"),
        "analyzer_version": status.raw.get("analyzer_version"),
        "sync": {
            "pending": status.pending_count,
            "state": "fresh" if status.is_fresh and status.pending_count == 0 else "pending",
            "fresh": status.is_fresh,
            "degraded_reason": status.degraded_reason,
        },
        "graph_degraded": bool(status.degraded_reason),
        "graph_degraded_reason": status.degraded_reason or "",
        "source": "devmap",
        **payload,
    }


def _require_usable(resp: BudgetedResponse, *, label: str) -> None:
    reason = resolution_unavailable_reason(resp.resolution)
    if reason:
        raise DevMapClientError(f"devmap {label} unavailable: {reason}")
    # Truncated-empty with a non-zero total is not a verified-zero answer.
    if resp.total > 0 and not resp.items and resp.truncated:
        raise DevMapClientError(
            f"devmap {label} returned truncated empty items (total={resp.total})"
        )


def _unavailable(
    root: Path, label: str, reason: str, empty: dict[str, Any]
) -> dict[str, Any]:
    """A tri-state *unavailable* answer, never a Python answer in disguise.

    The Rust kernel is primary. When it cannot answer, the honest result is
    "unavailable, and here is why" — not a silently substituted result from a
    second engine, and not a confident zero. Substituting was how SC23 hid for a
    whole pass: every consumer raised, quietly took the Python path, and
    reported success, so nothing in the output said which engine had answered.

    `resolution` carries the reason in the shape `resolution_unavailable_reason`
    already parses (X14), so existing consumers detect this without new code.
    The payload keys are present but empty, so a caller that reads them gets
    nothing rather than a KeyError — and `ok` is False so nobody mistakes the
    emptiness for a verified zero.
    """
    payload: dict[str, Any] = {
        "ok": False,
        "operation": label,
        "project_root": str(root.resolve()),
        "engine": "devmap-rust",
        "resolution": {"Unavailable": {"reason": f"{label}: {reason}"}},
        # Present on every envelope this module produces, so a consumer reading
        # it never has to test whether the key exists to find out whether the
        # walk finished. `_incomplete_walk_envelope` overrides it with the
        # kernel's own note when incompleteness is *why* this is unavailable.
        "walk_incomplete": None,
    }
    payload.update(empty)
    return payload


def _incomplete_walk_envelope(
    root: Path,
    label: str,
    resp: BudgetedResponse,
    rendered: list[Any],
    empty: dict[str, Any],
) -> dict[str, Any] | None:
    """The unavailable envelope for an empty answer from a walk that stopped early.

    Returns ``None`` when there is nothing to refuse — either the walk ran to
    completion, or it stopped early but still produced rows, in which case the
    caller keeps its rows and publishes the note beside them.

    This is the MCP half of the rule `walk_incomplete_reason` documents and the
    CLI already applies at ``graph_cmd._neighbor_edges``. It matters at the
    *default* depth: the kernel reports a capped reverse walk with
    ``truncated: false`` and ``hidden: 0``, so every counter on the response
    says "complete" while the walk withheld an unknown quantity. Published as
    ``[]`` that reads as "nothing depends on this", which is the answer that
    gets a live symbol deleted.
    """
    note = walk_incomplete_reason(resp)
    if not note or rendered:
        return None
    return _unavailable(
        root, label, f"walk incomplete: {note}", {**empty, "walk_incomplete": note}
    )
#: Coarse confidence names the Python engine published, and the score bands the
#: kernel's floats fall into. One owner: `dead` derived these inline and
#: `explore`'s blast radius needs the same vocabulary, and two copies would let
#: the same edge be `inferred` in one tool and `extracted` in another.
_CONFIDENCE_BANDS = ((0.9, "extracted"), (0.5, "inferred"))


def _confidence_tier(score: float) -> str:
    for floor, name in _CONFIDENCE_BANDS:
        if score >= floor:
            return name
    return "ambiguous"


#: Payload keys `explore` always carries, empty, so an unavailable answer is
#: readable by the same code that reads a successful one. `ok: False` and
#: `resolution` are what say it is not a measured zero.
_EXPLORE_EMPTY: dict[str, Any] = {
    "definitions": [],
    "match_count": 0,
    "match_total": 0,
    "matches_truncated": False,
    "blast_radius": {"seeds": [], "layers": [], "total_impacted": 0},
}

_AFFECTED_EMPTY: dict[str, Any] = {
    "tests": [],
    "test_details": [],
    "tests_total": 0,
    "tests_truncated": False,
    "blast_radius": {"seeds": [], "layers": [], "total_impacted": 0},
}


def _section_unavailable(section: dict[str, Any], label: str) -> str | None:
    """Why a budgeted section of a composed answer could not be produced.

    A section is unavailable when the kernel said so, and also when the walk
    behind it stopped early — those are different facts and both are carried,
    but only the first makes the section's zero meaningless.
    """
    reason = resolution_unavailable_reason(section.get("resolution"))
    return f"{label}: {reason}" if reason else None


def _blast_radius_payload(radius: dict[str, Any]) -> dict[str, Any]:
    """The kernel's banded radius in the shape consumers already read.

    Python published `{seeds, layers:[{depth, nodes, confidence, count}],
    total_impacted}` and every key survives. What is added is what Python could
    not say: which targets matched nothing, how many nodes a band held beyond
    the ones listed, and whether the walk itself was cut short.
    """
    layers_section = radius.get("layers") or {}
    layers = []
    for layer in layers_section.get("items") or []:
        score = layer.get("lowest_confidence")
        layers.append({
            "depth": layer.get("depth"),
            "nodes": layer.get("nodes") or [],
            "count": layer.get("node_count", 0),
            # A band with no edges has no confidence to report; naming one
            # would invent evidence. Python defaulted to "extracted" here,
            # which reads as measured.
            "confidence": _confidence_tier(float(score)) if isinstance(score, (int, float)) else None,
            "confidence_score": score,
            "nodes_omitted": layer.get("nodes_omitted", 0),
        })
    return {
        "seeds": radius.get("seeds") or [],
        "unmatched_targets": radius.get("unmatched_targets") or [],
        "layers": layers,
        "total_impacted": radius.get("total_impacted", 0),
        "layers_shown": layers_section.get("shown", 0),
        "layers_total": layers_section.get("total", 0),
        "layers_truncated": bool(layers_section.get("truncated")),
        "walk_incomplete": layers_section.get("walk_incomplete"),
        "unavailable": _section_unavailable(layers_section, "blast_radius"),
    }


def _edge_direction(section: dict[str, Any], label: str) -> dict[str, Any]:
    """One call-graph direction: its edges, its exact count, and why not more.

    `<side>_total` is the measured number of edges regardless of how many the
    budget bought, so "0 shown of 42" can never be read as "no callers".
    `<side>_unavailable` separates that again from a direction whose walk could
    not run at all.
    """
    return {
        label: section.get("items") or [],
        f"{label}_total": section.get("total", 0),
        f"{label}_truncated": bool(section.get("truncated")),
        f"{label}_shown": section.get("shown", 0),
        f"{label}_unavailable": _section_unavailable(section, label),
        f"{label}_walk_incomplete": section.get("walk_incomplete"),
    }


def _explore_via_client(root: Path, query: str, limit: int) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root, "explore", "no built devmap store (run `dev map`)", _EXPLORE_EMPTY
        )
    try:
        report = client.explore(query, limit=limit)
        definitions_section = report.get("definitions") or {}
        reason = _section_unavailable(definitions_section, "definitions")
        if reason:
            return _unavailable(root, "explore", reason, _EXPLORE_EMPTY)
        definitions = []
        for item in definitions_section.get("items") or []:
            span = item.get("span") or (0, 0)
            line = int(span[0]) if isinstance(span, (list, tuple)) and span else 0
            end_line = int(span[1]) if isinstance(span, (list, tuple)) and len(span) > 1 else line
            definitions.append({
                "id": item.get("id", ""),
                "kind": item.get("kind", "symbol"),
                "path": item.get("file_path", ""),
                "name": item.get("symbol_name", ""),
                "qualified_name": item.get("qualified_name", ""),
                "line": line,
                "end_line": end_line,
                # `None` means the generation recorded no language for the
                # file, which is not the same as a file with no language.
                "language": item.get("language"),
                "source": item.get("source", ""),
                # Class A: an unreadable file is not a symbol with an empty
                # body, and a capped span is not the verbatim source.
                "source_unavailable_reason": item.get("source_unavailable_reason"),
                "source_omitted_bytes": item.get("source_omitted_bytes"),
                "score": item.get("score"),
                **_edge_direction(item.get("callers") or {}, "callers"),
                **_edge_direction(item.get("callees") or {}, "callees"),
            })
        return _client_envelope(
            root,
            client,
            {
                "query": query,
                "definitions": definitions,
                # The legacy names keep their legacy meanings: `match_count` is
                # what came back, `match_total` what the index holds.
                "match_count": definitions_section.get("shown", len(definitions)),
                "match_total": definitions_section.get("total", 0),
                "matches_truncated": bool(definitions_section.get("truncated")),
                "limit_applied": limit,
                "blast_radius": _blast_radius_payload(report.get("blast_radius") or {}),
                "budget": report.get("budget") or {},
            },
            operation="explore",
        )
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) explore failed: %s. The kernel is the only engine; "
            "this answers unavailable rather than substituting another.",
            exc,
        )
        return _unavailable(root, "explore", str(exc), _EXPLORE_EMPTY)


def _affected_via_client(root: Path, targets: list[str], max_depth: int) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root, "affected_tests", "no built devmap store (run `dev map`)", _AFFECTED_EMPTY
        )
    if not targets:
        # Not an empty answer: nothing was asked, so nothing was measured.
        return _unavailable(
            root, "affected_tests", "no targets were supplied", _AFFECTED_EMPTY
        )
    try:
        report = client.affected_tests(targets, depth=max(1, min(64, max_depth)))
        tests_section = report.get("tests") or {}
        reason = _section_unavailable(tests_section, "tests")
        if reason:
            return _unavailable(root, "affected_tests", reason, _AFFECTED_EMPTY)
        rows = tests_section.get("items") or []
        return _client_envelope(
            root,
            client,
            {
                "targets": targets,
                # The contract shape: a list of paths. `test_details` carries
                # the distance and the symbols the walk actually reached, which
                # is what makes a trimmed list orderable rather than arbitrary.
                "tests": [str(row.get("path", "")) for row in rows],
                "test_details": rows,
                "tests_total": tests_section.get("total", 0),
                "tests_truncated": bool(tests_section.get("truncated")),
                "tests_walk_incomplete": tests_section.get("walk_incomplete"),
                "blast_radius": _blast_radius_payload(report.get("blast_radius") or {}),
            },
            operation="affected_tests",
        )
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) affected_tests failed: %s. The kernel is the only "
            "engine; this answers unavailable rather than substituting another.",
            exc,
        )
        return _unavailable(root, "affected_tests", str(exc), _AFFECTED_EMPTY)


def _hit_to_match(item: dict[str, Any]) -> dict[str, Any]:
    path = str(item.get("file_path") or item.get("path") or "")
    name = str(item.get("symbol_name") or item.get("name") or "")
    span = item.get("span") or (0, 0)
    line = int(span[0]) if isinstance(span, (list, tuple)) and span else int(item.get("line") or 0)
    kind = str(item.get("kind") or "symbol")
    node_id = str(item.get("id") or (f"{path}::{name}" if path and name else name or path))
    return {
        "id": node_id,
        "path": path,
        "name": name,
        "kind": kind,
        "line": line,
        "score": item.get("score"),
        "source_span": item.get("source_span") or "",
    }


def _edge_to_layer_nodes(items: list[dict[str, Any]]) -> list[str]:
    nodes: list[str] = []
    seen: set[str] = set()
    for item in items:
        source = str(item.get("source_symbol") or item.get("source_file") or "")
        if not source or source in seen:
            continue
        seen.add(source)
        nodes.append(source)
    return nodes


def _search_via_client(root: Path, query: str, limit: int) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root,
            "search",
            "no built devmap store (run `dev map`)",
            {"matches": [], "shown": 0, "total": 0, "truncated": False},
        )
    try:
        resp = client.search(query, limit=2000)
        _require_usable(resp, label="search")
        matches = [_hit_to_match(item) for item in resp.items[: max(1, limit)]]
        capped = _incomplete_walk_envelope(
            root, "search", resp, matches,
            {"query": query, "matches": [], "shown": 0, "total": resp.total, "truncated": resp.truncated},
        )
        if capped is not None:
            return capped
        return _client_envelope(
            root,
            client,
            {
                "query": query,
                "matches": matches,
                "shown": len(matches),
                "total": resp.total,
                "truncated": resp.truncated or len(resp.items) > limit,
                "walk_incomplete": walk_incomplete_reason(resp),
            },
            operation="search",
        )
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) search failed and this call fell back to the Python "
            "path: %s. The Rust kernel is primary; a fallback here is a defect.",
            exc,
        )
        return _unavailable(root, "search", str(exc), {"matches": [], "shown": 0, "total": 0, "truncated": False})


def _path_via_client(root: Path, start: str, end: str, max_depth: int) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root,
            "path",
            "no built devmap store (run `dev map`)",
            {"found": False, "path": [], "length": 0, "truncated": False},
        )
    try:
        depth = max(1, min(64, max_depth))
        resp = client.trace(start, depth=depth, to_symbol=end)
        reason = resolution_unavailable_reason(resp.resolution)
        if reason:
            # "The index could not answer" is not "there is no path". This
            # branch used to return `_client_envelope`, whose `ok` is True, so
            # the two arrived at the caller as the same `found: false` — and
            # with `operation` left at "", the very key `_client_envelope`'s
            # docstring names as the gap a fallback once hid. Every sibling
            # operation routes an unavailable resolution through `_unavailable`;
            # this one now does too.
            return _unavailable(
                root,
                "path",
                reason,
                {"from": start, "to": end, "found": False, "path": [], "length": 0, "truncated": False},
            )
        # A trace capped to nothing with work still outstanding is not a
        # verified absence either; `_require_usable` raises, and the handler
        # below turns that into the same unavailable answer.
        _require_usable(resp, label="path")
        steps = []
        for item in resp.items:
            node = str(
                item.get("node")
                or item.get("symbol_name")
                or item.get("target_symbol")
                or item.get("source_symbol")
                or ""
            )
            if node:
                steps.append({"node": node, "via": item if isinstance(item, dict) else None})
        # "The walk stopped before it reached the target" is not "there is no
        # path"; `found: false` from a capped traversal is the same confident
        # zero the Unavailable branch above refuses.
        capped = _incomplete_walk_envelope(
            root, "path", resp, steps,
            {"from": start, "to": end, "found": False, "path": [], "length": 0,
             "truncated": resp.truncated},
        )
        if capped is not None:
            return capped
        return _client_envelope(
            root,
            client,
            {
                "from": start,
                "to": end,
                "found": bool(steps),
                "length": max(0, len(steps) - 1) if steps else 0,
                "path": steps,
                "truncated": resp.truncated,
                "walk_incomplete": walk_incomplete_reason(resp),
            },
            operation="path",
        )
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) path failed and this call fell back to the Python "
            "path: %s. The Rust kernel is primary; a fallback here is a defect.",
            exc,
        )
        return _unavailable(root, "path", str(exc), {"found": False, "path": [], "length": 0, "truncated": False})


def _impact_via_client(root: Path, targets: list[str], max_depth: int) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root,
            "impact",
            "no built devmap store (run `dev map`)",
            {"nodes": [], "layers": [], "count": 0, "total_impacted": 0, "truncated": False},
        )
    try:
        depth = max(1, min(8, max_depth))
        layers_nodes: list[str] = []
        seeds: list[str] = []
        truncated = False
        incomplete_notes: list[str] = []
        for target in targets:
            resp = client.impact(str(target), depth=depth)
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                raise DevMapClientError(f"impact unavailable for {target}: {reason}")
            _require_usable(resp, label=f"impact:{target}")
            seeds.append(str(target))
            nodes = _edge_to_layer_nodes(resp.items)
            # Per target, not per union: a seed whose own walk stopped with
            # nothing to show contributes an unknown, and a union that silently
            # absorbs it is "nothing depends on this file" from a walk that
            # stopped looking.
            note = walk_incomplete_reason(resp)
            if note and not nodes:
                return _unavailable(
                    root,
                    "impact",
                    f"walk incomplete for {target}: {note}",
                    {"targets": targets, "nodes": [], "layers": [], "count": 0,
                     "total_impacted": 0, "truncated": resp.truncated,
                     "walk_incomplete": note},
                )
            if note:
                incomplete_notes.append(f"{target}: {note}")
            layers_nodes.extend(nodes)
            truncated = truncated or resp.truncated
        # Preserve envelope shape expected by MCP consumers.
        unique_nodes = sorted(set(layers_nodes))
        walk_note = "; ".join(incomplete_notes) or None
        blast = {
            "seeds": seeds,
            "layers": [{
                "depth": 1,
                "nodes": unique_nodes,
                "confidence": "extracted",
                "count": len(unique_nodes),
            }] if unique_nodes else [],
            "total_impacted": len(unique_nodes),
            "truncated": truncated,
            # A non-empty radius keeps its nodes and still says the walk was
            # capped, so `total_impacted` is read as a floor rather than a total.
            "walk_incomplete": walk_note,
        }
        return _client_envelope(
            root,
            client,
            {"targets": targets, "blast_radius": blast, "walk_incomplete": walk_note},
            operation="impact",
        )
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) impact failed and this call fell back to the Python "
            "path: %s. The Rust kernel is primary; a fallback here is a defect.",
            exc,
        )
        return _unavailable(
            root,
            "impact",
            str(exc),
            {"nodes": [], "layers": [], "count": 0, "total_impacted": 0, "truncated": False},
        )


def _dead_via_client(root: Path, minimum_confidence: str) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root,
            "dead",
            "no built devmap store (run `dev map`)",
            {"dead_code": [], "runtime_proven_live": [], "total": 0, "truncated": False},
        )
    try:
        resp = client.dead_symbols(budget=2000)
        _require_usable(resp, label="dead")
        ranks = {"ambiguous": 0, "inferred": 1, "extracted": 2}
        floor = ranks.get(minimum_confidence, 1)
        rows: list[dict[str, Any]] = []
        for item in resp.items:
            confidence = str(item.get("confidence") or item.get("tier") or "inferred")
            # Rust may emit numeric confidence; map coarsely for filtering.
            if isinstance(item.get("confidence"), (int, float)):
                confidence = _confidence_tier(float(item["confidence"]))
            if ranks.get(confidence, 0) < floor:
                continue
            path = str(item.get("file_path") or item.get("path") or "")
            name = str(item.get("symbol_name") or item.get("name") or "")
            span = item.get("span") or (0, 0)
            line = int(span[0]) if isinstance(span, (list, tuple)) and span else int(item.get("line") or 0)
            rows.append({
                **item,
                "id": str(item.get("id") or (f"{path}::{name}" if path and name else name or path)),
                "path": path,
                "name": name,
                "line": line,
                "confidence": confidence,
                "tier": (
                    "high-confidence dead candidate"
                    if confidence == "extracted"
                    else "unconfirmed/unwired"
                ),
            })
        capped = _incomplete_walk_envelope(
            root, "dead", resp, rows,
            {"minimum_confidence": minimum_confidence, "dead_code": [],
             "runtime_proven_live": [], "total": resp.total, "truncated": resp.truncated},
        )
        if capped is not None:
            return capped
        status = client.status()
        return _client_envelope(
            root,
            client,
            {
                "minimum_confidence": minimum_confidence,
                "dead_code": rows,
                "runtime_proven_live": [],
                "index_freshness": {
                    "fresh": status.is_fresh,
                    "generation": status.generation_id,
                    "pending_count": status.pending_count,
                    "reason": status.degraded_reason,
                },
                "truncated": resp.truncated,
                "total": resp.total,
                "walk_incomplete": walk_incomplete_reason(resp),
            },
            operation="dead",
        )
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) dead failed and this call fell back to the Python "
            "path: %s. The Rust kernel is primary; a fallback here is a defect.",
            exc,
        )
        return _unavailable(
            root,
            "dead",
            str(exc),
            {"dead_code": [], "runtime_proven_live": [], "total": 0, "truncated": False},
        )


def _status_via_client(root: Path) -> dict[str, Any]:
    client = try_connect(root)
    if client is None:
        return _unavailable(
            root,
            "status",
            "no built devmap store (run `dev map`)",
            # For *status* specifically, an unbuilt store is not a failure to
            # report around — it is the answer the caller asked for, and the
            # Python engine named it `uninitialized`. Keeping that word keeps
            # the contract while `resolution` still records why the Rust kernel
            # had nothing further to say.
            {
                "state": "uninitialized",
                "generation": None,
                "node_count": 0,
                "edge_count": 0,
                "pending_count": 0,
                "is_fresh": False,
                "degraded_reason": None,
            },
        )
    try:
        status = client.status()
        state = "committed" if status.generation_id > 0 else "empty"
        return {
            "ok": True,
            # Built inline rather than through `_client_envelope`, so the
            # operation key has to be set here too — its absence is exactly the
            # gap the fallback used to hide.
            "operation": "status",
            "project_root": str(root.resolve()),
            "state": state,
            "generation": status.generation_id or None,
            "node_count": status.node_count,
            "edge_count": status.edge_count,
            "pending_count": status.pending_count,
            "is_fresh": status.is_fresh,
            "degraded_reason": status.degraded_reason,
            "quarantined_count": status.quarantined_count,
            "source": "devmap",
            "sync": {
                "pending": status.pending_count,
                "state": "fresh" if status.is_fresh and status.pending_count == 0 else "pending",
                "fresh": status.is_fresh,
                "degraded_reason": status.degraded_reason,
            },
            **{k: v for k, v in status.raw.items() if k not in {
                "generation_id", "pending_count", "node_count", "edge_count", "is_fresh"
            }},
        }
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) status failed and this call fell back to the Python "
            "path: %s. The Rust kernel is primary; a fallback here is a defect.",
            exc,
        )
        return _unavailable(
            root,
            "status",
            str(exc),
            {
                "state": "unavailable",
                "generation": None,
                "node_count": 0,
                "edge_count": 0,
                "pending_count": 0,
                "is_fresh": False,
                "degraded_reason": str(exc),
            },
        )


# Every handler below hands its blocking body to a worker thread.
#
# All of them are synchronous end to end — a devmap IPC exchange (socket reads
# bounded at RESPONSE_DEADLINE_SECONDS = 30, a daemon-readiness loop calling
# time.sleep(0.05), and a CLI fallback at subprocess.run(timeout=120)) or a
# SQLite graph load. Run inline they park the event loop for all of it, which is
# the failure `util.run_cli_command` already documents: while the loop is parked
# the server stops answering `ping` and cannot even receive the
# `notifications/cancelled` a client sends to give up. That reasoning was
# applied to the CLI helper and to nothing else. Offloading changes only *where*
# the work runs, never what it answers.


async def _explore(root: Path, arguments: dict) -> list[TextContent]:
    # The Rust kernel answers, or says it cannot. The Python
    # `CodeIntelQueryEngine` that used to answer here loaded a whole `CodeGraph`
    # into process memory and walked it — and since the Python writer was
    # retired there is no store for it to load, so it raised
    # `FileNotFoundError` on every call and this tool reported
    # `codeintel_not_initialized` against a freshly built map.
    return json_text(
        await asyncio.to_thread(
            _explore_via_client, root, str(arguments["query"]), int(arguments.get("limit", 20))
        )
    )


async def _search(root: Path, arguments: dict) -> list[TextContent]:
    query = str(arguments["query"])
    limit = int(arguments.get("limit", 50))
    # The Rust kernel answers, or says it cannot. It never hands off to
    # `CodeIntelQueryEngine`: a second engine answering with no signal
    # which one answered is the SC23 shape, and an unavailable answer a
    # caller can see beats a confident one from an engine it did not ask
    # for.
    return json_text(await asyncio.to_thread(_search_via_client, root, query, limit))


async def _path(root: Path, arguments: dict) -> list[TextContent]:
    start = str(arguments["from"])
    end = str(arguments["to"])
    max_depth = int(arguments.get("maxDepth", 32))
    # The Rust kernel answers, or says it cannot. It never hands off to
    # `CodeIntelQueryEngine`: a second engine answering with no signal
    # which one answered is the SC23 shape, and an unavailable answer a
    # caller can see beats a confident one from an engine it did not ask
    # for.
    return json_text(await asyncio.to_thread(_path_via_client, root, start, end, max_depth))


async def _impact(root: Path, arguments: dict) -> list[TextContent]:
    targets = [str(value) for value in arguments.get("targets") or []]
    max_depth = int(arguments.get("maxDepth", 3))
    # The Rust kernel answers, or says it cannot. It never hands off to
    # `CodeIntelQueryEngine`: a second engine answering with no signal
    # which one answered is the SC23 shape, and an unavailable answer a
    # caller can see beats a confident one from an engine it did not ask
    # for.
    return json_text(await asyncio.to_thread(_impact_via_client, root, targets, max_depth))


async def _dead(root: Path, arguments: dict) -> list[TextContent]:
    minimum = str(arguments.get("minimumConfidence", "inferred"))
    # The Rust kernel answers, or says it cannot. It never hands off to
    # `CodeIntelQueryEngine`: a second engine answering with no signal
    # which one answered is the SC23 shape, and an unavailable answer a
    # caller can see beats a confident one from an engine it did not ask
    # for.
    return json_text(await asyncio.to_thread(_dead_via_client, root, minimum))


async def _affected(root: Path, arguments: dict) -> list[TextContent]:
    # Kernel-only, on the same terms as its six siblings above.
    return json_text(
        await asyncio.to_thread(
            _affected_via_client,
            root,
            [str(value) for value in arguments.get("targets") or []],
            int(arguments.get("maxDepth", 3)),
        )
    )


async def _sync(root: Path, arguments: dict) -> list[TextContent]:
    """Build through the kernel — over IPC when the daemon answers, else the CLI.

    There is no Python fallback. This used to hand off to the Python
    ``SyncCoordinator``, which re-extracted with the retired Python engine and
    rewrote both artifacts; the kernel is the only writer, so an unreachable
    kernel is reported as ``engine_unavailable`` rather than answered by a
    second engine the caller never asked for.
    """
    supplied = [str(value) for value in arguments.get("paths") or []]

    def _via_daemon() -> dict[str, Any] | None:
        """Connect, build and read status in one worker thread, or None."""
        client = try_connect(root)
        if client is None:
            return None
        result = client.build(affected=supplied or None)
        status = client.status()
        return {
            "ok": True,
            "reconciled": supplied,
            "source": "devmap",
            "generation": result.get("generation_id", status.generation_id),
            "pending": status.pending_count,
            "state": "fresh" if status.is_fresh else "pending",
            "fresh": status.is_fresh,
            "build": result,
        }

    try:
        built = await asyncio.to_thread(_via_daemon)
    except DevMapClientError as exc:
        logger.warning(
            "devmap (Rust) daemon build failed; retrying through the kernel CLI: %s",
            exc,
        )
    else:
        if built is not None:
            return json_text(built)
    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    map_path = root / ".devcouncil" / "repo_map.json"
    try:
        refresh = await asyncio.to_thread(refresh_map_artifacts, root, map_path, quiet=True)
    except DevMapEngineError as exc:
        return error_text(str(exc), code="engine_unavailable", reconciled=supplied)
    kernel = getattr(refresh, "kernel_status", None)
    return json_text({
        "ok": True,
        "reconciled": supplied,
        "source": "devmap-cli",
        "generation": refresh.generation,
        "pending": kernel.pending_count if kernel is not None else None,
        "state": (
            "fresh" if (kernel is not None and kernel.is_fresh)
            else "pending" if kernel is not None
            else "unknown"
        ),
        "fresh": kernel.is_fresh if kernel is not None else None,
        "build": {"mode": refresh.mode, "reason": refresh.reason},
    })


async def _status(root: Path, _arguments: dict) -> list[TextContent]:
    # `_status_via_client` always answers — `committed`, `empty`, or
    # `uninitialized` with the reason carried in `resolution`. The Python
    # service is not consulted, because reporting *its* store's state under a
    # question about the Rust kernel is how a caller ends up confident about an
    # index that was never built.
    return json_text(await asyncio.to_thread(_status_via_client, root))


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
        # An unknown tool name is not an unavailable *answer* — this server
        # simply does not implement that tool, and `None` is how the MCP layer
        # says so. Returning a tri-state envelope here would claim the tool
        # exists but could not run.
        return None
    try:
        root = resolve_root(default_root, arguments)
    except ProjectPathOutsideRoot as exc:
        # A distinct code, not `invalid_arguments`: the argument was well-formed
        # and the server refused it, and a caller that cannot tell those apart
        # will retry the same path forever. Listed first — it is a ValueError.
        return error_text(str(exc), code="project_path_outside_root", tool=name)
    except (OSError, ValueError) as exc:
        # An embedded NUL or an over-long component makes the OS refuse to stat
        # the path, so containment cannot be decided. Unanswerable is not
        # allowed, so this is a refusal with its reason rather than an
        # unhandled error at the transport boundary.
        return error_text(
            f"projectPath is not a usable path: {exc}",
            code="invalid_arguments", tool=name, argument="projectPath",
        )
    try:
        if name in {"devcouncil_code_sync", "devcouncil_code_status"}:
            return await handler(root, arguments)
        return await with_codeintel_freshness(root, lambda: handler(root, arguments))
    except FileNotFoundError as exc:
        return error_text(str(exc), code="codeintel_not_initialized", project_root=str(root))
    except (KeyError, TypeError, ValueError) as exc:
        return error_text(str(exc), code="invalid_arguments", tool=name)
