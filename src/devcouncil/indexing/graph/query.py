"""Query helpers over a persisted CodeGraph.

devcouncil: allow-unwired — package-private; reached via package ``__init__`` / CLI.
"""

from __future__ import annotations

from collections import defaultdict, deque
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

from devcouncil.indexing.graph.build import load_code_graph
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode


def _load(root: Path, graph: Optional[CodeGraph] = None) -> Optional[CodeGraph]:
    return graph if graph is not None else load_code_graph(root)


def _match_nodes(graph: CodeGraph, name_or_path: str) -> List[GraphNode]:
    q = name_or_path.replace("\\", "/")
    hits: List[GraphNode] = []
    for n in graph.nodes:
        if n.id == q or n.path == q or n.name == q:
            hits.append(n)
            continue
        if q in n.id or n.path.endswith(q) or n.id.endswith(f"::{q}"):
            hits.append(n)
    return hits


def query_symbol(
    root: Path,
    name_or_path: str,
    *,
    graph: Optional[CodeGraph] = None,
) -> Dict[str, Any]:
    """360° view: definition, callers, callees, importers."""
    g = _load(root, graph)
    if g is None:
        return {"error": "no code graph; run `dev map` first", "query": name_or_path}
    nodes = _match_nodes(g, name_or_path)
    if not nodes:
        return {"query": name_or_path, "matches": [], "definitions": []}

    callers: Dict[str, List[str]] = defaultdict(list)
    callees: Dict[str, List[str]] = defaultdict(list)
    importers: Dict[str, List[str]] = defaultdict(list)
    imports: Dict[str, List[str]] = defaultdict(list)
    for e in g.edges:
        if e.kind == "calls":
            callers[e.target].append(e.source)
            callees[e.source].append(e.target)
        elif e.kind == "imports":
            importers[e.target].append(e.source)
            imports[e.source].append(e.target)

    results = []
    for n in nodes[:20]:
        results.append(
            {
                "id": n.id,
                "kind": n.kind.value if hasattr(n.kind, "value") else n.kind,
                "path": n.path,
                "name": n.name,
                "line": n.line,
                "area": n.area,
                "callers": sorted(set(callers.get(n.id, [])))[:50],
                "callees": sorted(set(callees.get(n.id, [])))[:50],
                "importers": sorted(set(importers.get(n.id, [])))[:50],
                "imports": sorted(set(imports.get(n.id, [])))[:50],
            }
        )
    return {"query": name_or_path, "matches": len(nodes), "definitions": results}


def trace_path(
    root: Path,
    start: str,
    end: str,
    *,
    graph: Optional[CodeGraph] = None,
    max_depth: int = 32,
) -> Dict[str, Any]:
    """Shortest path between two nodes (by id/name/path) over imports+calls+contains."""
    g = _load(root, graph)
    if g is None:
        return {"error": "no code graph; run `dev map` first", "from": start, "to": end}

    starts = _match_nodes(g, start)
    ends = _match_nodes(g, end)
    if not starts or not ends:
        return {
            "from": start,
            "to": end,
            "path": [],
            "found": False,
            "reason": "endpoint not found",
        }

    start_ids = {n.id for n in starts}
    end_ids = {n.id for n in ends}
    adj: Dict[str, Set[str]] = defaultdict(set)
    for e in g.edges:
        if e.kind in {"imports", "calls", "contains", "defines", "inherits"}:
            adj[e.source].add(e.target)
            adj[e.target].add(e.source)  # undirected for reachability UX

    # BFS from all starts
    queue: deque = deque()
    parent: Dict[str, Optional[str]] = {}
    for s in start_ids:
        queue.append(s)
        parent[s] = None
    found: Optional[str] = None
    depth = 0
    while queue and depth < max_depth:
        for _ in range(len(queue)):
            cur = queue.popleft()
            if cur in end_ids:
                found = cur
                break
            for nxt in adj.get(cur, ()):
                if nxt not in parent:
                    parent[nxt] = cur
                    queue.append(nxt)
        if found:
            break
        depth += 1

    if not found:
        return {"from": start, "to": end, "path": [], "found": False}

    path: List[str] = []
    walk: Optional[str] = found
    while walk is not None:
        path.append(walk)
        walk = parent.get(walk)
    path.reverse()
    # Prefer a path that starts at a start_id
    return {"from": start, "to": end, "path": path, "found": True, "length": len(path) - 1}
