"""Community labeling helpers (leaf module — no intel/verifier imports)."""

from __future__ import annotations

from collections import Counter

from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind


def _is_file_node(n: GraphNode) -> bool:
    kind = n.kind.value if hasattr(n.kind, "value") else str(n.kind)
    return kind == NodeKind.FILE.value or ("::" not in n.id and not n.name)


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
