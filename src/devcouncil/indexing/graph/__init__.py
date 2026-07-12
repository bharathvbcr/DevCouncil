"""Symbol-level code knowledge graph for DevCouncil repo mapping.

Public entry points: :func:`build_code_graph`, :func:`refresh_map_for_paths`,
:func:`load_code_graph`, :func:`query_symbol`, :func:`trace_path`, plus
graph intelligence (:func:`enrich_graph_intel`, :func:`diff_impact`, …).
"""

from __future__ import annotations

from devcouncil.indexing.graph.build import (
    build_code_graph,
    load_code_graph,
    refresh_map_for_paths,
    write_code_graph,
)
from devcouncil.indexing.graph.export import (
    build_code_graph_okf,
    export_graphml,
    file_doc_rel,
    write_code_graph_okf,
)
from devcouncil.indexing.graph.intel import (
    blast_radius,
    circular_imports,
    compute_communities,
    diff_impact,
    enrich_graph_intel,
    extract_processes,
    god_nodes,
    graph_check,
)
from devcouncil.indexing.graph.query import query_symbol, trace_path
from devcouncil.indexing.graph.schema import (
    SCHEMA_VERSION,
    CodeGraph,
    Confidence,
    DeadCodeEntry,
    GraphEdge,
    GraphNode,
    NodeKind,
)

__all__ = [
    "SCHEMA_VERSION",
    "CodeGraph",
    "Confidence",
    "DeadCodeEntry",
    "GraphEdge",
    "GraphNode",
    "NodeKind",
    "blast_radius",
    "build_code_graph",
    "build_code_graph_okf",
    "circular_imports",
    "compute_communities",
    "diff_impact",
    "enrich_graph_intel",
    "export_graphml",
    "extract_processes",
    "file_doc_rel",
    "god_nodes",
    "graph_check",
    "load_code_graph",
    "query_symbol",
    "refresh_map_for_paths",
    "trace_path",
    "write_code_graph",
    "write_code_graph_okf",
]


def __getattr__(name: str):
    """Lazy OKF facade symbols (avoid import cycles with repo_mapper)."""
    if name in {"build_graph_okf_bundle", "export_graph_okf", "graph_to_graphml"}:
        from devcouncil.indexing.graph import okf_export

        return getattr(okf_export, name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
