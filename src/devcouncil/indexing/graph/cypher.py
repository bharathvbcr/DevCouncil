"""Minimal openCypher-style subset over the native in-memory code graph."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

_SUPPORTED_REL = frozenset({
    "CALLS", "IMPORTS", "CONTAINS", "REFERENCES", "IMPLEMENTS", "EXTENDS", "DECORATES",
})

#: Ceiling on the row count a single query may materialise. The `LIMIT n` in a
#: query is user-supplied text, not a budget the server has to honour: without
#: a bound, `LIMIT 999999999` builds a row dict per edge in the graph. It
#: matches the store's own search ceiling so the two surfaces agree.
_MAX_ROW_LIMIT = 500

_MATCH_RE = re.compile(
    r"^\s*MATCH\s+\(a(?::(?P<alabel>[\w]+))?\)"
    r"(?:-\[r:(?P<rel>[\w|]+)\]->\(b(?::(?P<blabel>[\w]+))?\))?"
    r"(?:\s+WHERE\s+(?P<where>.+?))?"
    r"\s+RETURN\s+(?P<ret>.+?)(?:\s+LIMIT\s+(?P<limit>\d+))?\s*$",
    re.IGNORECASE | re.DOTALL,
)


def _parse_where(clause: str) -> Tuple[Optional[str], Optional[str]]:
    if not clause:
        return None, None
    lowered = clause.strip().lower()
    if "a.name" in lowered and "contains" in lowered:
        m = re.search(r"contains\s*\(\s*a\.name\s*,\s*['\"]([^'\"]+)['\"]\s*\)", clause, re.I)
        if m:
            return m.group(1), None
    if "b.path" in lowered and "starts with" in lowered:
        m = re.search(r"starts\s+with\s*\(\s*b\.path\s*,\s*['\"]([^'\"]+)['\"]\s*\)", clause, re.I)
        if m:
            return None, m.group(1)
    return None, None


def run_cypher(project_root: Path, query: str, *, default_limit: int = 50) -> Dict[str, Any]:
    """Execute a supported MATCH … RETURN subset; reject unsupported clauses."""
    normalized = " ".join(query.split())
    if re.search(r"\b(CREATE|DELETE|MERGE|SET|REMOVE|DETACH)\b", normalized, re.I):
        return {"ok": False, "error": "Mutating Cypher clauses are not supported."}
    m = _MATCH_RE.match(normalized)
    if not m:
        return {
            "ok": False,
            "error": (
                "Unsupported Cypher. Supported: "
                "MATCH (a)-[r:CALLS|IMPORTS|...]->(b) WHERE … RETURN a,b LIMIT n"
            ),
        }
    rels = m.group("rel") or ""
    rel_types = {r.strip().upper() for r in rels.split("|") if r.strip()}
    for rel in rel_types:
        if rel not in _SUPPORTED_REL:
            return {"ok": False, "error": f"Unsupported relationship type: {rel}"}
    requested_limit = int(m.group("limit") or default_limit)
    limit = max(1, min(_MAX_ROW_LIMIT, requested_limit))
    name_filter, path_prefix = _parse_where(m.group("where") or "")

    from devcouncil.codeintel.service import get_codeintel_service

    try:
        # The service owns the store, the root and the runtime-observation
        # table; this used to reach through a query engine's private `_graph`
        # for all three. Same graph, same runtime merge, same failure mode —
        # asked of the owner instead.
        graph = get_codeintel_service(project_root).load_with_runtime_observations()
    except FileNotFoundError:
        return {"ok": False, "error": "No committed graph generation."}

    nodes_by_id = {n.id: n for n in graph.nodes}
    rows: List[Dict[str, Any]] = []
    # Counted over every match, not just the ones that fit. The loops used to
    # break out at the cap, so `count` equalled the cap and read like a total:
    # a query over 5000 edges and one over exactly 50 were indistinguishable.
    total = 0

    if rel_types:
        for edge in graph.edges:
            if edge.kind.upper() not in rel_types:
                continue
            a = nodes_by_id.get(edge.source)
            b = nodes_by_id.get(edge.target)
            if a is None or b is None:
                continue
            if name_filter and name_filter.lower() not in a.name.lower():
                continue
            if path_prefix and not b.path.startswith(path_prefix):
                continue
            total += 1
            if len(rows) >= limit:
                continue
            rows.append({
                "a_id": a.id,
                "a_name": a.name,
                "a_path": a.path,
                "a_kind": a.kind,
                "rel": edge.kind,
                "b_id": b.id,
                "b_name": b.name,
                "b_path": b.path,
                "b_kind": b.kind,
            })
    else:
        for node in graph.nodes:
            if name_filter and name_filter.lower() not in node.name.lower():
                continue
            total += 1
            if len(rows) >= limit:
                continue
            rows.append({
                "a_id": node.id,
                "a_name": node.name,
                "a_path": node.path,
                "a_kind": node.kind,
            })

    return {
        "ok": True,
        "rows": rows,
        # `count` keeps its original meaning — rows returned — and the numbers
        # that make it interpretable now travel with it.
        "count": len(rows),
        "shown": len(rows),
        "total": total,
        "truncated": len(rows) < total,
        "limit_requested": requested_limit,
        "limit_applied": limit,
        "limit_capped": limit != requested_limit,
    }
