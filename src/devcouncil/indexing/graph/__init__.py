"""Symbol-level code knowledge graph for DevCouncil repo mapping.

Public entry points: :func:`load_code_graph`, :func:`write_code_graph`,
:func:`query_symbol`, :func:`trace_path`, plus graph intelligence
(:func:`enrich_graph_intel`, :func:`diff_impact`, …).

Neither building the graph nor refreshing the artifacts is one of them. The Rust
kernel extracts and resolves the graph and writes both ``.devcouncil/repo_map.json``
and ``.devcouncil/graph/code_graph.json``, through
:func:`devcouncil.indexing.map_artifacts.refresh_map_artifacts`.

**The facade is lazy, and has to stay lazy.** Importing any submodule of a
package runs this file first, so an eager ``from .build import …`` here is paid
by every importer of every submodule — including the two on the `dev map` hot
path that want one cheap leaf each: ``repo_mapper`` imports
``graph.cache.PARSE_CACHE_VERSION`` (one integer) and ``map_artifacts`` imports
``graph.schema.CodeGraph`` (one type annotation). Eagerly re-exporting the
surface cost them **136.6 ms** cumulative on this repository, measured with
``python -X importtime`` on `dev map`, of which ``graph.build`` alone was
125.7 ms. Deferring the re-export leaves the facade identical to an importer
that actually wants it — the first attribute access imports the submodule and
caches the value in this module's globals — and free for one that does not.
``tests/unit/test_graph_package_import_cost.py`` holds the line.
"""

from __future__ import annotations

from importlib import import_module
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:  # pragma: no cover - import-time cost is the whole point
    # Type checkers resolve the facade statically; only the interpreter defers.
    from devcouncil.indexing.graph.build import (  # noqa: F401
        load_code_graph,
        write_code_graph,
    )
    from devcouncil.indexing.graph.export import (  # noqa: F401
        build_code_graph_okf,
        export_graphml,
        file_doc_rel,
        write_code_graph_okf,
    )
    from devcouncil.indexing.graph.intel import (  # noqa: F401
        blast_radius,
        circular_imports,
        compute_communities,
        diff_impact,
        enrich_graph_intel,
        extract_processes,
        god_nodes,
        graph_check,
    )
    from devcouncil.indexing.graph.okf_export import (  # noqa: F401
        build_graph_okf_bundle,
        export_graph_okf,
        graph_to_graphml,
    )
    from devcouncil.indexing.graph.query import (  # noqa: F401
        query_symbol,
        trace_path,
    )
    from devcouncil.indexing.graph.schema import (  # noqa: F401
        SCHEMA_VERSION,
        CodeGraph,
        Confidence,
        DeadCodeEntry,
        GraphEdge,
        GraphNode,
        NodeKind,
    )

#: Public name -> the submodule that defines it. The single source of truth for
#: both the deferral and ``__all__``; a name added to one is added to both.
_EXPORTS: dict[str, str] = {
    "load_code_graph": "build",
    "write_code_graph": "build",
    "build_code_graph_okf": "export",
    "export_graphml": "export",
    "file_doc_rel": "export",
    "write_code_graph_okf": "export",
    "blast_radius": "intel",
    "circular_imports": "intel",
    "compute_communities": "intel",
    "diff_impact": "intel",
    "enrich_graph_intel": "intel",
    "extract_processes": "intel",
    "god_nodes": "intel",
    "graph_check": "intel",
    "query_symbol": "query",
    "trace_path": "query",
    "SCHEMA_VERSION": "schema",
    "CodeGraph": "schema",
    "Confidence": "schema",
    "DeadCodeEntry": "schema",
    "GraphEdge": "schema",
    "GraphNode": "schema",
    "NodeKind": "schema",
    # The OKF facade was already deferred, to avoid an import cycle with
    # `repo_mapper` rather than for cost. Both reasons now share one mechanism.
    "build_graph_okf_bundle": "okf_export",
    "export_graph_okf": "okf_export",
    "graph_to_graphml": "okf_export",
}

__all__ = sorted(_EXPORTS)


def __getattr__(name: str) -> Any:
    """Import the defining submodule on first access, then cache the value.

    The value is written into this module's globals so only the first access
    pays the lookup; every later one is an ordinary global read, and
    ``vars(graph)`` reports what has actually been loaded.
    """
    submodule = _EXPORTS.get(name)
    if submodule is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    value = getattr(import_module(f"{__name__}.{submodule}"), name)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted({*globals(), *__all__})
