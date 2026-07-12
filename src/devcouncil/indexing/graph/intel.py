"""Graph intelligence: communities, god nodes, cycles, processes, diff impact.

LLM-free analysis over a :class:`CodeGraph` (NetworkX Louvain + BFS).
"""

from __future__ import annotations

import logging
from collections import Counter, defaultdict, deque
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Set, Tuple

from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind

logger = logging.getLogger(__name__)

_STRUCTURAL_KINDS = frozenset({"imports", "calls"})
_FILE_EDGE_KINDS = frozenset({"imports"})
_CALL_KINDS = frozenset({"calls"})


def _is_file_node(n: GraphNode) -> bool:
    kind = n.kind.value if hasattr(n.kind, "value") else str(n.kind)
    return kind == NodeKind.FILE.value or ("::" not in n.id and not n.name)


def _file_id(path_or_id: str) -> str:
    return path_or_id.replace("\\", "/").split("::", 1)[0]


def _path_prefix_label(paths: Sequence[str], *, depth: int = 2) -> str:
    """Dominant directory prefix among ``paths`` (LLM-free community label)."""
    counts: Counter[str] = Counter()
    for p in paths:
        norm = p.replace("\\", "/").strip("/")
        if not norm:
            continue
        parts = norm.split("/")
        if len(parts) >= depth:
            counts["/".join(parts[:depth])] += 1
        elif parts:
            counts[parts[0]] += 1
    if not counts:
        return "community"
    return counts.most_common(1)[0][0]


def compute_communities(
    graph: CodeGraph,
    *,
    seed: int = 0,
) -> Dict[str, Any]:
    """Louvain communities over import+call edges; label from dominant path prefixes.

    Mutates ``graph.nodes`` in place, setting ``community``. Returns a summary dict
    suitable for ``graph.meta["communities"]``.
    """
    try:
        from networkx.algorithms.community import louvain_communities
    except ImportError:
        logger.warning("networkx missing; skipping communities")
        return {"communities": [], "count": 0}

    # Prefer file-level import/call collapse for stable, path-labelable clusters.
    file_edges: List[Tuple[str, str]] = []
    seen: Set[Tuple[str, str]] = set()
    for e in graph.edges:
        if e.kind not in _STRUCTURAL_KINDS:
            continue
        a, b = _file_id(e.source), _file_id(e.target)
        if a == b:
            continue
        key = (a, b) if a < b else (b, a)
        if key in seen:
            continue
        seen.add(key)
        file_edges.append((a, b))

    import networkx as nx

    G = nx.Graph()
    G.add_edges_from(file_edges)
    # Include isolated file nodes so every file gets a community id.
    for n in graph.nodes:
        if _is_file_node(n) and n.id not in G:
            G.add_node(_file_id(n.id))

    if G.number_of_nodes() == 0:
        return {"communities": [], "count": 0}

    raw = louvain_communities(G, seed=seed, weight=None)
    # Deterministic order: sort communities by sorted member list.
    ordered = sorted(
        (sorted(c) for c in raw),
        key=lambda members: (members[0] if members else ""),
    )

    id_to_path = {_file_id(n.id): (n.path or _file_id(n.id)) for n in graph.nodes}
    communities: List[Dict[str, Any]] = []
    file_to_label: Dict[str, str] = {}
    used_labels: Counter[str] = Counter()

    for idx, members in enumerate(ordered):
        paths = [id_to_path.get(m, m) for m in members]
        base = _path_prefix_label(paths)
        used_labels[base] += 1
        label = base if used_labels[base] == 1 else f"{base}#{used_labels[base]}"
        for m in members:
            file_to_label[m] = label
        communities.append(
            {
                "id": idx,
                "label": label,
                "size": len(members),
                "files": members[:40],
            }
        )

    for n in graph.nodes:
        fid = _file_id(n.path or n.id)
        n.community = file_to_label.get(fid, "")

    return {"communities": communities, "count": len(communities)}


def pagerank_scores(graph: CodeGraph) -> Dict[str, float]:
    """PageRank over the directed import+call graph (empty when networkx absent)."""
    try:
        import networkx as nx
    except ImportError:
        return {}
    G = nx.DiGraph()
    for e in graph.edges:
        if e.kind in _STRUCTURAL_KINDS and e.source != e.target:
            G.add_edge(e.source, e.target)
    if G.number_of_nodes() == 0:
        return {}
    try:
        return dict(nx.pagerank(G, alpha=0.85, max_iter=100))
    except Exception:
        logger.debug("pagerank failed", exc_info=True)
        return {}


def god_nodes(graph: CodeGraph, *, top_n: int = 15) -> List[Dict[str, Any]]:
    """Top-connected symbols/files: degree, fan-in/fan-out split, PageRank."""
    degree: Counter[str] = Counter()
    fan_in: Counter[str] = Counter()
    fan_out: Counter[str] = Counter()
    for e in graph.edges:
        if e.kind not in _STRUCTURAL_KINDS:
            continue
        degree[e.source] += 1
        degree[e.target] += 1
        fan_out[e.source] += 1
        fan_in[e.target] += 1
    pr = pagerank_scores(graph)
    by_id = graph.node_by_id()
    out: List[Dict[str, Any]] = []
    for nid, deg in degree.most_common(top_n):
        n = by_id.get(nid)
        out.append(
            {
                "id": nid,
                "degree": deg,
                "fan_in": fan_in.get(nid, 0),
                "fan_out": fan_out.get(nid, 0),
                "pagerank": round(pr.get(nid, 0.0), 6),
                "kind": (
                    n.kind.value if n and hasattr(n.kind, "value") else (n.kind if n else "")
                ),
                "path": n.path if n else _file_id(nid),
                "name": n.name if n else "",
                "community": n.community if n else "",
            }
        )
    return out


def hotspots(
    root: Path,
    graph: CodeGraph,
    *,
    since: str = "90.days",
    top_n: int = 20,
) -> List[Dict[str, Any]]:
    """Churn × coupling hotspots: files changed often AND heavily depended on.

    Score = commits touching the file (last ``since``) × (1 + fan-in).
    High scores are refactor-risk files where a change ripples widest.
    """
    import math

    try:
        from devcouncil.utils.proc import git_output

        raw = git_output(
            ["log", f"--since={since}", "--name-only", "--pretty=format:"],
            cwd=root,
            default="",
        )
    except Exception:
        logger.debug("hotspot churn scan failed", exc_info=True)
        return []
    churn: Counter[str] = Counter()
    for line in raw.splitlines():
        p = line.strip().replace("\\", "/")
        if p:
            churn[p] += 1
    if not churn:
        return []

    fan_in: Counter[str] = Counter()
    for e in graph.edges:
        if e.kind in _FILE_EDGE_KINDS and "::" not in e.source and "::" not in e.target:
            fan_in[e.target] += 1

    file_paths = {n.path or n.id for n in graph.nodes if _is_file_node(n)}
    scored: List[Dict[str, Any]] = []
    for path, count in churn.items():
        if path not in file_paths:
            continue
        fi = fan_in.get(path, 0)
        scored.append(
            {
                "path": path,
                "churn": count,
                "fan_in": fi,
                "score": round(count * (1 + math.log1p(fi)), 2),
            }
        )
    scored.sort(key=lambda h: (-h["score"], h["path"]))
    return scored[:top_n]


def circular_imports(graph: CodeGraph, *, max_cycles: int = 50) -> List[Dict[str, Any]]:
    """Detect circular import chains among file nodes."""
    try:
        import networkx as nx
    except ImportError:
        return []

    G = nx.DiGraph()
    for e in graph.edges:
        if e.kind not in _FILE_EDGE_KINDS:
            continue
        src, tgt = e.source, e.target
        if "::" in src or "::" in tgt:
            continue
        if src != tgt:
            G.add_edge(src, tgt)

    cycles: List[Dict[str, Any]] = []
    try:
        for cycle in nx.simple_cycles(G):
            if len(cycle) < 2:
                continue
            start = min(range(len(cycle)), key=lambda i: cycle[i])
            rotated = cycle[start:] + cycle[:start]
            cycles.append({"nodes": rotated, "length": len(rotated)})
            if len(cycles) >= max_cycles:
                break
    except Exception:
        logger.debug("cycle detection failed", exc_info=True)
        return []

    cycles.sort(key=lambda c: (c["length"], c["nodes"]))
    return cycles


def graph_check(graph: CodeGraph, *, top_n: int = 15) -> Dict[str, Any]:
    """God nodes + circular imports report (``dev graph check``)."""
    gods = god_nodes(graph, top_n=top_n)
    cycles = circular_imports(graph)
    return {
        "god_nodes": gods,
        "circular_imports": cycles,
        "god_count": len(gods),
        "cycle_count": len(cycles),
    }


def _call_adjacency(graph: CodeGraph) -> Dict[str, List[str]]:
    adj: Dict[str, List[str]] = defaultdict(list)
    for e in graph.edges:
        if e.kind in _CALL_KINDS:
            adj[e.source].append(e.target)
    return {k: sorted(set(v)) for k, v in adj.items()}


def _entry_symbols(graph: CodeGraph, entry_root: str) -> List[str]:
    """Symbol ids defined in an entry-root file (prefer functions named main/run/cli)."""
    root = entry_root.replace("\\", "/")
    symbols: List[GraphNode] = []
    for n in graph.nodes:
        if _is_file_node(n):
            continue
        if (n.path or _file_id(n.id)) == root:
            symbols.append(n)
    if not symbols:
        return [root] if any(n.id == root for n in graph.nodes) else []

    preferred = {"main", "run", "cli", "app", "handler", "execute"}
    ranked = sorted(
        symbols,
        key=lambda n: (
            0 if n.name in preferred else 1,
            0
            if (n.kind.value if hasattr(n.kind, "value") else str(n.kind)) == "function"
            else 1,
            n.line,
            n.id,
        ),
    )
    return [n.id for n in ranked[:5]]


def extract_processes(
    graph: CodeGraph,
    *,
    entry: Optional[str] = None,
    max_depth: int = 6,
    max_processes: int = 20,
    max_steps: int = 24,
) -> List[Dict[str, Any]]:
    """BFS call-flows from entry roots (named, step-ordered, depth-capped)."""
    adj = _call_adjacency(graph)
    roots = list(graph.entry_roots)
    if entry:
        q = entry.replace("\\", "/")
        roots = [r for r in roots if r == q or r.endswith(q) or q in r]
        if not roots:
            roots = [q]

    processes: List[Dict[str, Any]] = []
    for root in roots:
        starts = _entry_symbols(graph, root)
        if not starts and root:
            starts = [root]
        for start in starts:
            steps: List[str] = []
            seen: Set[str] = set()
            queue: deque[Tuple[str, int]] = deque([(start, 0)])
            while queue and len(steps) < max_steps:
                node, depth = queue.popleft()
                if node in seen:
                    continue
                seen.add(node)
                steps.append(node)
                if depth >= max_depth:
                    continue
                for nxt in adj.get(node, []):
                    if nxt not in seen:
                        queue.append((nxt, depth + 1))
            name = f"{Path(root).name}"
            if start != root and "::" in start:
                name = f"{Path(root).name}::{start.split('::', 1)[-1]}"
            processes.append(
                {
                    "name": name,
                    "entry": start,
                    "entry_root": root,
                    "steps": steps,
                    "depth": max(0, len(steps) - 1),
                }
            )
            if len(processes) >= max_processes:
                return processes
    processes.sort(key=lambda p: (-p["depth"], -len(p["steps"]), p["name"]))
    return processes[:max_processes]


def _enclosing_symbols(graph: CodeGraph, path: str) -> List[GraphNode]:
    norm = path.replace("\\", "/")
    hits = [
        n
        for n in graph.nodes
        if not _is_file_node(n) and (n.path or _file_id(n.id)) == norm
    ]
    return sorted(hits, key=lambda n: (n.line, n.id))


def _inbound_adj(graph: CodeGraph) -> Dict[str, List[str]]:
    """Callers/importers → reverse edges for blast radius."""
    rev: Dict[str, List[str]] = defaultdict(list)
    for e in graph.edges:
        if e.kind in _STRUCTURAL_KINDS:
            rev[e.target].append(e.source)
    return {k: sorted(set(v)) for k, v in rev.items()}


def blast_radius(
    graph: CodeGraph,
    seed_ids: Sequence[str],
    *,
    max_depth: int = 3,
) -> Dict[str, Any]:
    """Inbound blast radius at depths 1/2/3 with confidence tiers."""
    rev = _inbound_adj(graph)
    by_depth: Dict[int, List[str]] = {1: [], 2: [], 3: []}
    confidence_for_depth = {1: "extracted", 2: "inferred", 3: "ambiguous"}
    seen: Set[str] = set(seed_ids)
    frontier = set(seed_ids)
    for depth in range(1, max_depth + 1):
        nxt: Set[str] = set()
        for nid in frontier:
            for caller in rev.get(nid, []):
                if caller in seen:
                    continue
                seen.add(caller)
                nxt.add(caller)
                by_depth[depth].append(caller)
        by_depth[depth] = sorted(by_depth[depth])
        frontier = nxt
        if not frontier:
            break

    layers = []
    for d in range(1, max_depth + 1):
        layers.append(
            {
                "depth": d,
                "confidence": confidence_for_depth[d],
                "nodes": by_depth[d],
                "count": len(by_depth[d]),
            }
        )
    return {
        "seeds": list(seed_ids),
        "layers": layers,
        "total_impacted": sum(len(by_depth[d]) for d in by_depth),
    }


def working_tree_changed_paths(root: Path) -> List[str]:
    """Repository-relative paths changed in the working tree (unstaged+staged)."""
    try:
        from devcouncil.verification.verifier import Verifier

        return [p.replace("\\", "/") for p in Verifier(root).get_changed_files()]
    except Exception:
        logger.debug("working tree paths failed", exc_info=True)
        return []


def diff_impact(
    root: Path,
    graph: CodeGraph,
    *,
    paths: Optional[Sequence[str]] = None,
    use_diff: bool = False,
    max_depth: int = 3,
) -> Dict[str, Any]:
    """Map paths (or working-tree diff) → enclosing symbols → inbound blast radius."""
    if use_diff or not paths:
        changed = working_tree_changed_paths(root)
        if paths:
            want = {p.replace("\\", "/") for p in paths}
            changed = [p for p in changed if p in want]
        target_paths = changed
    else:
        target_paths = [p.replace("\\", "/") for p in paths]

    items: List[Dict[str, Any]] = []
    all_seeds: List[str] = []
    for path in target_paths:
        symbols = _enclosing_symbols(graph, path)
        seed_ids = [s.id for s in symbols] or [path]
        all_seeds.extend(seed_ids)
        radius = blast_radius(graph, seed_ids, max_depth=max_depth)
        items.append(
            {
                "path": path,
                "symbols": [
                    {
                        "id": s.id,
                        "name": s.name,
                        "line": s.line,
                        "kind": s.kind.value if hasattr(s.kind, "value") else str(s.kind),
                    }
                    for s in symbols[:40]
                ],
                "blast": radius,
            }
        )

    aggregate = (
        blast_radius(graph, all_seeds, max_depth=max_depth)
        if all_seeds
        else {"seeds": [], "layers": [], "total_impacted": 0}
    )
    return {
        "paths": items,
        "aggregate": aggregate,
        "path_count": len(items),
        "source": "diff" if (use_diff or not paths) else "paths",
    }


def enrich_graph_intel(
    graph: CodeGraph, *, root: Optional[Path] = None, seed: int = 0
) -> CodeGraph:
    """Run communities + processes + centrality + hotspots; persist to ``graph.meta``."""
    community_summary = compute_communities(graph, seed=seed)
    processes = extract_processes(graph)
    meta = dict(graph.meta or {})
    meta["communities"] = community_summary
    # Per-node community map so downstream consumers (viz) never have to
    # re-derive it from mutated node attributes.
    meta["node_communities"] = {
        n.id: n.community for n in graph.nodes if n.community
    }
    meta["processes"] = processes[:12]
    meta["god_nodes"] = god_nodes(graph)[:15]
    meta["circular_imports"] = circular_imports(graph)[:30]
    if root is not None:
        meta["hotspots"] = hotspots(root, graph)
    graph.meta = meta
    return graph


def community_label_for_area(graph: CodeGraph, area: str) -> str:
    """Dominant community label among file nodes under ``area`` (generic subsystems)."""
    counts: Counter[str] = Counter()
    prefix = area.replace("\\", "/").rstrip("/") + "/"
    for n in graph.nodes:
        if not _is_file_node(n):
            continue
        path = (n.path or n.id).replace("\\", "/")
        if path == area or path.startswith(prefix):
            if n.community:
                counts[n.community] += 1
    if not counts:
        return ""
    return counts.most_common(1)[0][0]
