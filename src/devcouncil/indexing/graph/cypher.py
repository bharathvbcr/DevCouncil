"""Minimal openCypher-style subset over the native in-memory code graph."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Dict, List, NamedTuple, Optional, Tuple

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


#: The whole vocabulary of the WHERE subset: one anchored pattern per term, and
#: the field each one filters. Anchored because a term has to be recognised
#: *entirely* — matching a fragment somewhere inside `NOT contains(a.name, 'x')`
#: would apply the opposite filter and report success.
_WHERE_TERMS: Tuple[Tuple[str, "re.Pattern[str]"], ...] = (
    ("name", re.compile(r"^contains\s*\(\s*a\.name\s*,\s*['\"]([^'\"]+)['\"]\s*\)$", re.I)),
    ("path", re.compile(r"^starts\s+with\s*\(\s*b\.path\s*,\s*['\"]([^'\"]+)['\"]\s*\)$", re.I)),
)


class WhereClause(NamedTuple):
    """The filters a WHERE clause asked for, and the terms nobody could read.

    ``unparsed`` is the point of this type. The parser used to return only the
    filters it understood, so a clause it did not understand at all — ``a.kind =
    'function'``, ``b.path STARTS WITH 'src/secret'`` in infix form, anything
    joined by ``OR`` — came back as "no filters", and the query then matched
    *every* row. The caller received more rows than it asked for and nothing in
    the payload said the filter had not run. A filter that could not be
    evaluated must never report as one that ran and matched.
    """

    name_filter: Optional[str]
    path_prefix: Optional[str]
    unparsed: Tuple[str, ...]


def parse_where(clause: str) -> WhereClause:
    """Split a WHERE clause into recognised filters and unrecognised terms.

    Terms are conjunctions: ``A AND B``. Anything else — an ``OR``, a ``NOT``,
    an infix comparison, a field this subset does not index — lands in
    ``unparsed`` rather than being dropped. ``OR`` in particular cannot be
    honoured by an engine that only conjoins filters, so silently accepting it
    would run a *different* query than the one asked for.
    """
    if not clause or not clause.strip():
        return WhereClause(None, None, ())
    name_filter: Optional[str] = None
    path_prefix: Optional[str] = None
    unparsed: List[str] = []
    for term in re.split(r"\s+AND\s+", clause.strip(), flags=re.I):
        text = term.strip()
        if not text:
            continue
        for field, pattern in _WHERE_TERMS:
            m = pattern.match(text)
            if not m:
                continue
            if field == "name":
                name_filter = m.group(1)
            else:
                path_prefix = m.group(1)
            break
        else:
            unparsed.append(text)
    return WhereClause(name_filter, path_prefix, tuple(unparsed))


def _parse_where(clause: str) -> Tuple[Optional[str], Optional[str]]:
    """The two-filter view of :func:`parse_where`, for callers that have already
    established the clause is fully understood."""
    parsed = parse_where(clause)
    return parsed.name_filter, parsed.path_prefix


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
    where = parse_where(m.group("where") or "")
    if where.unparsed:
        # Refused, not silently widened. Dropping an unreadable filter returns
        # every row in the graph under `ok: true`, which is a strictly wrong
        # answer to the question asked — the same class as a filter that cannot
        # be evaluated reporting as one that ran and matched.
        return {
            "ok": False,
            "code": "unsupported_where",
            "error": (
                "Unsupported WHERE term(s): "
                + "; ".join(where.unparsed)
                + ". Supported: contains(a.name, '...') and "
                "starts with(b.path, '...'), joined by AND. Refusing rather "
                "than returning unfiltered rows."
            ),
            "unparsed_where": list(where.unparsed),
        }
    name_filter, path_prefix = where.name_filter, where.path_prefix

    from devcouncil.codeintel.query.engine import CodeIntelQueryEngine

    try:
        graph = CodeIntelQueryEngine(project_root)._graph()
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
