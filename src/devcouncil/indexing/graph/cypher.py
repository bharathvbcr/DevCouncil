"""Minimal openCypher-style subset over the native in-memory code graph."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

_SUPPORTED_REL = frozenset({
    "CALLS", "IMPORTS", "CONTAINS", "REFERENCES", "IMPLEMENTS", "EXTENDS", "DECORATES",
})

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
    limit = int(m.group("limit") or default_limit)
    name_filter, path_prefix = _parse_where(m.group("where") or "")

    from devcouncil.codeintel.query.engine import CodeIntelQueryEngine

    try:
        graph = CodeIntelQueryEngine(project_root)._graph()
    except FileNotFoundError:
        return {"ok": False, "error": "No committed graph generation."}

    nodes_by_id = {n.id: n for n in graph.nodes}
    rows: List[Dict[str, Any]] = []

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
            if len(rows) >= limit:
                break
    else:
        for node in graph.nodes:
            if name_filter and name_filter.lower() not in node.name.lower():
                continue
            rows.append({
                "a_id": node.id,
                "a_name": node.name,
                "a_path": node.path,
                "a_kind": node.kind,
            })
            if len(rows) >= limit:
                break

    return {"ok": True, "rows": rows, "count": len(rows)}
