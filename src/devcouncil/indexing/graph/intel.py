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
# Ambiguous call/import edges fan out to every candidate and inflate hubs,
# PageRank, process walks, and blast radius. Keep them in the store for
# explanation UIs, but exclude them from those ranked/BFS surfaces.
_METRIC_CONFIDENCE = frozenset({"extracted", "inferred"})


def _edge_confidence(edge: object) -> str:
    conf = getattr(edge, "confidence", None)
    if conf is None:
        return "extracted"
    return conf.value if hasattr(conf, "value") else str(conf)


def _metric_edge(edge: object) -> bool:
    """True when an edge should influence god-node / PageRank / process / impact."""
    kind = getattr(edge, "kind", "")
    if kind not in _STRUCTURAL_KINDS:
        return False
    return _edge_confidence(edge) in _METRIC_CONFIDENCE


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
        file_edges.append(key)

    import networkx as nx

    G = nx.Graph()
    G.add_edges_from(sorted(file_edges))
    # Include isolated file nodes so every file gets a community id.
    isolated_file_ids = sorted(
        {_file_id(n.id) for n in graph.nodes if _is_file_node(n)}
    )
    for node_id in isolated_file_ids:
        if node_id not in G:
            G.add_node(node_id)

    if G.number_of_nodes() == 0:
        return {"communities": [], "count": 0}

    # Bound Louvain so pathological graphs cannot stall a full map/graph build.
    timeout_seconds = 15.0

    def _run_louvain() -> list:
        return list(louvain_communities(G, seed=seed, weight=None))

    from concurrent.futures import ThreadPoolExecutor, TimeoutError as FuturesTimeout

    try:
        with ThreadPoolExecutor(max_workers=1) as pool:
            raw = pool.submit(_run_louvain).result(timeout=timeout_seconds)
    except FuturesTimeout:
        logger.warning(
            "community detection timed out after %.1fs (%d nodes); skipping",
            timeout_seconds,
            G.number_of_nodes(),
        )
        return {
            "communities": [],
            "count": 0,
            "skipped": True,
            "reason": f"louvain_timeout_{timeout_seconds:.0f}s",
        }
    except Exception:
        logger.warning("community detection failed; skipping", exc_info=True)
        return {"communities": [], "count": 0, "skipped": True, "reason": "louvain_error"}
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


def _pagerank_power_iteration(
    edges: List[tuple],
    *,
    alpha: float = 0.85,
    max_iter: int = 100,
    tol: float = 1.0e-8,
) -> Dict[str, float]:
    """Deterministic pure-Python PageRank (networkx semantics, no numpy/scipy).

    networkx's pagerank imports scipy→numpy at call time; installs without them
    raise and previously zeroed every score. Fine for code-graph sizes.
    """
    nodes = sorted({n for pair in edges for n in pair})
    if not nodes:
        return {}
    out_links: Dict[str, List[str]] = {n: [] for n in nodes}
    for src, dst in edges:
        out_links[src].append(dst)
    count = len(nodes)
    rank = dict.fromkeys(nodes, 1.0 / count)
    for _ in range(max_iter):
        prev = rank
        dangling = sum(prev[n] for n in nodes if not out_links[n])
        base = (1.0 - alpha) / count + alpha * dangling / count
        rank = dict.fromkeys(nodes, base)
        for src in nodes:
            targets = out_links[src]
            if not targets:
                continue
            share = alpha * prev[src] / len(targets)
            for dst in targets:
                rank[dst] += share
        if sum(abs(rank[n] - prev[n]) for n in nodes) < count * tol:
            break
    return rank


def pagerank_scores(graph: CodeGraph) -> Dict[str, float]:
    """PageRank over the directed import+call graph.

    Prefers networkx; falls back to a pure-Python power iteration when
    networkx (or its numpy/scipy solver deps) is unavailable. Ambiguous
    edges are excluded so name-collision fan-out cannot dominate ranks.
    """
    structural = sorted(
        {
            (e.source, e.target)
            for e in graph.edges
            if _metric_edge(e) and e.source != e.target
        }
    )
    if not structural:
        return {}
    raw: Dict[str, float] = {}
    try:
        import networkx as nx

        G = nx.DiGraph()
        G.add_edges_from(structural)
        raw = dict(nx.pagerank(G, alpha=0.85, max_iter=100, tol=1.0e-8))
    except Exception:
        logger.debug(
            "networkx pagerank unavailable; using pure-Python fallback", exc_info=True
        )
        raw = _pagerank_power_iteration(structural)
    # Fixed precision so tiny solver/order noise cannot flip JSON bytes.
    return {nid: round(float(score), 4) for nid, score in raw.items()}


def god_nodes(graph: CodeGraph, *, top_n: int = 15) -> List[Dict[str, Any]]:
    """Top-connected production symbols/files: degree, fan-in/fan-out, PageRank.

    Test-path nodes are excluded from the ranking — shared mocks and fixture
    helpers accumulate huge fan-in and would otherwise crowd out real hubs.
    Ambiguous edges are excluded for the same reason (collision fan-out).
    """
    from devcouncil.indexing.wiring import is_test_path

    degree: Counter[str] = Counter()
    fan_in: Counter[str] = Counter()
    fan_out: Counter[str] = Counter()
    for e in graph.edges:
        if not _metric_edge(e):
            continue
        degree[e.source] += 1
        degree[e.target] += 1
        fan_out[e.source] += 1
        fan_in[e.target] += 1
    pr = pagerank_scores(graph)
    by_id = graph.node_by_id()

    def _is_test_node(nid: str) -> bool:
        n = by_id.get(nid)
        return is_test_path(n.path if n and n.path else _file_id(nid))

    ranked = sorted(
        ((nid, deg) for nid, deg in degree.items() if not _is_test_node(nid)),
        key=lambda item: (-item[1], item[0]),
    )[:top_n]
    out: List[Dict[str, Any]] = []
    for nid, deg in ranked:
        n = by_id.get(nid)
        out.append(
            {
                "id": nid,
                "degree": deg,
                "fan_in": fan_in.get(nid, 0),
                "fan_out": fan_out.get(nid, 0),
                "pagerank": pr.get(nid, 0.0),
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


def _is_package_init(path: str) -> bool:
    return Path(path.replace("\\", "/")).name == "__init__.py"


def _import_components(
    graph: CodeGraph,
    *,
    include_package_inits: bool,
) -> List[Dict[str, Any]]:
    """Deterministic strongly connected components in the file import graph."""
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
        if not include_package_inits and (
            _is_package_init(src) or _is_package_init(tgt)
        ):
            continue
        if src != tgt:
            G.add_edge(src, tgt)

    try:
        components = [
            sorted(component)
            for component in nx.strongly_connected_components(G)
            if len(component) >= 2
        ]
    except Exception:
        logger.debug("cycle detection failed", exc_info=True)
        return []

    components.sort(key=lambda nodes: (len(nodes), nodes))
    return [{"nodes": nodes, "length": len(nodes)} for nodes in components]


def circular_imports(graph: CodeGraph, *, max_cycles: int = 50) -> List[Dict[str, Any]]:
    """Report actionable import SCCs, excluding Python package-init barrel noise."""
    return _import_components(graph, include_package_inits=False)[:max_cycles]


def graph_check(graph: CodeGraph, *, top_n: int = 15) -> Dict[str, Any]:
    """God nodes + circular imports report (``dev graph check``)."""
    gods = god_nodes(graph, top_n=top_n)
    cycles = circular_imports(graph)
    package_init_components = [
        component
        for component in _import_components(graph, include_package_inits=True)
        if any(_is_package_init(node) for node in component["nodes"])
    ]
    return {
        "god_nodes": gods,
        "circular_imports": cycles,
        "package_init_imports": package_init_components,
        "god_count": len(gods),
        "cycle_count": len(cycles),
        "package_init_count": len(package_init_components),
    }


def _call_adjacency(graph: CodeGraph) -> Dict[str, List[str]]:
    adj: Dict[str, List[str]] = defaultdict(list)
    for e in graph.edges:
        if e.kind in _CALL_KINDS and _edge_confidence(e) in _METRIC_CONFIDENCE:
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
    """Callers/importers → reverse edges for blast radius.

    Ambiguous edges are omitted so collision fan-out does not inflate impact.
    """
    rev: Dict[str, List[str]] = defaultdict(list)
    for e in graph.edges:
        if _metric_edge(e):
            rev[e.target].append(e.source)
    return {k: sorted(set(v)) for k, v in rev.items()}


def blast_radius(
    graph: CodeGraph,
    seed_ids: Sequence[str],
    *,
    max_depth: int = 3,
) -> Dict[str, Any]:
    """Inbound blast radius at depths 1/2/3 with confidence tiers.

    Traversal follows extracted/inferred import+call edges only. Layer labels
    remain depth-based (depth 1 extracted … depth 3 ambiguous) as a distance
    heuristic, not a claim about edge confidence.
    """
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
        from devcouncil.verification.git_diff_fallback import GitDiffFallback

        return [p.replace("\\", "/") for p in GitDiffFallback(root).get_changed_files()]
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
    meta["package_init_import_count"] = sum(
        any(_is_package_init(node) for node in component["nodes"])
        for component in _import_components(graph, include_package_inits=True)
    )
    if root is not None:
        meta["hotspots"] = hotspots(root, graph)
    graph.meta = meta
    return graph


# Re-export leaf helper for callers that still import from intel.
from devcouncil.indexing.graph.communities import community_label_for_area  # noqa: E402,F401
