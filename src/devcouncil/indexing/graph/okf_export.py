"""Compatibility facade for code-graph OKF export.

Prefers the shared implementation in :mod:`devcouncil.indexing.graph.export`
and re-exports helpers used by wiki link conventions.

devcouncil: allow-unwired — thin re-export surface; callers use graph.export directly.
"""

from __future__ import annotations

from pathlib import Path
from typing import List

from devcouncil.indexing.graph.export import (
    build_code_graph_okf,
    export_graphml,
    file_doc_rel,
    write_code_graph_okf,
)
from devcouncil.indexing.graph.export_links import (
    GRAPH_FROM_WIKI,
    WIKI_FROM_GRAPH,
    cross_bundle_file_link,
    file_doc_path,
    relative_md_link,
    slugify_area,
    subsystem_doc_path,
    wired_to_bullets,
)
from devcouncil.indexing.graph.schema import CodeGraph

# Public aliases matching the Phase-7 plan names
build_graph_okf_bundle = build_code_graph_okf
graph_to_graphml = export_graphml


def export_graph_okf(
    graph: CodeGraph,
    out_dir: Path,
    *,
    root: Path | None = None,
    project_name: str = "Code Graph",
) -> List[Path]:
    """Write an OKF v0.1 bundle for ``graph`` under ``out_dir``."""
    base = root or Path(".")
    _out, written = write_code_graph_okf(
        base, out_dir, graph=graph, project_name=project_name
    )
    return written


__all__ = [
    "GRAPH_FROM_WIKI",
    "WIKI_FROM_GRAPH",
    "build_graph_okf_bundle",
    "cross_bundle_file_link",
    "export_graph_okf",
    "file_doc_path",
    "file_doc_rel",
    "graph_to_graphml",
    "relative_md_link",
    "slugify_area",
    "subsystem_doc_path",
    "wired_to_bullets",
]
