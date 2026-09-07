"""Query helpers over a persisted CodeGraph.

devcouncil: allow-unwired — package-private; reached via package ``__init__`` / CLI.

`query_symbol` and `trace_path` used to live here as the fallback engine for
`dev map query` / `dev map trace` and their MCP siblings. They are gone: the
kernel is the only graph engine, a missing kernel is an error naming it, and in
`trace`'s case the Python answer was not merely slower but *different* — an
undirected BFS over `imports`/`calls`/`contains`/`defines`/`inherits` that
reported paths the directional walk correctly finds no evidence for.

What remains reads the graph for questions the kernel does not answer in this
shape: the opt-in PDG layer, and `symbol_has_non_test_inbound` for the dead-code
gates.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Optional

from devcouncil.indexing.graph.build import read_code_graph
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode


def _load(root: Path, graph: Optional[CodeGraph] = None) -> Optional[CodeGraph]:
    """The kernel's `code_graph.json`, or the graph a caller already holds.

    This was `load_code_graph`, which reaches the same data through the Python
    `index.sqlite` cache. The dead-symbol gate below is the reason that matters:
    it runs as a *fallback for when the kernel cannot be reached*, and its own
    caller documents it as reading "code_graph.json, the artifact the same
    kernel writes". It did not — it imported that artifact into a second store
    first, ~94 MB of writes under a writer lease, from a verification gate.
    """
    return graph if graph is not None else read_code_graph(root)


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


def symbol_has_non_test_inbound(
    root: Path,
    path: str,
    name: str,
    *,
    graph: Optional[CodeGraph] = None,
) -> bool:
    """Return whether a symbol has a non-structural inbound production edge.

    Structural ownership edges (``contains``/``defines``) exist for every
    declaration and therefore do not prove wiring. Test-only callers also do not
    clear a production dead-symbol finding.
    """
    from devcouncil.indexing.wiring import is_test_path

    loaded = _load(root, graph)
    if loaded is None:
        return False
    normalized = path.replace("\\", "/")
    targets = {
        node.id
        for node in loaded.nodes
        if node.path.replace("\\", "/") == normalized and node.name == name
    }
    if not targets:
        return False
    by_id = {node.id: node for node in loaded.nodes}
    structural = {"contains", "defines", "documents"}
    for edge in loaded.edges:
        if edge.target not in targets or edge.kind in structural or edge.source in targets:
            continue
        source = by_id.get(edge.source)
        source_path = source.path if source is not None else edge.source.split("::", 1)[0]
        if source_path and not is_test_path(source_path):
            return True
    return False


# --- Opt-in PDG query helpers ---


#: Said by every PDG surface when the opt-in layer has not been built.
_NO_PDG = (
    "no PDG layer at .devcouncil/graph/pdg.json; run `dev map --pdg` or "
    "`dev map pdg build` first"
)


def _match_pdg_functions(layer, target: str):
    """Functions in the built layer matching ``target``: a path or a name.

    The layer is the only input. This used to resolve a bare name to files by
    scanning ``graph.nodes`` — a whole-graph read to answer a question the PDG
    layer's own ``qualname``s answer, since a function with no PDG entry cannot
    be in the result either way.
    """
    q = target.replace("\\", "/")
    hits = []
    for path, file_pdg in layer.files.items():
        if q.endswith(".py") and path.replace("\\", "/") != q:
            continue
        for fn in file_pdg.functions:
            if (
                q.endswith(".py")
                or fn.qualname == q
                or fn.qualname.endswith(f".{q}")
                or q in fn.qualname
            ):
                hits.append(fn)
    return hits


def _pdg_layer(root: Path):
    from devcouncil.indexing.graph.build import read_pdg_layer_file

    return read_pdg_layer_file(root)


def explain_pdg_taint(
    root: Path,
    *,
    graph: Optional[CodeGraph] = None,
    path: Optional[str] = None,
    category: Optional[str] = None,
) -> Dict[str, Any]:
    """Taint findings from the built PDG layer.

    Complete, not sampled. The findings used to come from ``graph.meta["pdg"]``,
    where ``PDGLayer.to_meta`` had trimmed them to the first 500 with nothing in
    the payload saying so — a capped list published as the answer to "what
    reaches a sink". The sidecar carries every function's findings, and
    ``count`` counts them.
    """
    layer = _pdg_layer(root)
    if layer is None:
        return {"ok": False, "error": _NO_PDG}
    findings = list(layer.taint_findings)
    if path:
        path = path.replace("\\", "/")
        findings = [f for f in findings if f.path == path]
    if category:
        findings = [f for f in findings if f.category == category]
    return {"ok": True, "count": len(findings), "findings": [f.to_dict() for f in findings]}


def query_pdg_controls(
    root: Path,
    target: str,
    *,
    graph: Optional[CodeGraph] = None,
) -> Dict[str, Any]:
    layer = _pdg_layer(root)
    if layer is None:
        return {"ok": False, "error": _NO_PDG}
    functions = _match_pdg_functions(layer, target)
    if not functions:
        return {"ok": False, "error": f"no PDG for target {target!r}; run `dev map pdg build`"}
    return {
        "ok": True,
        "target": target,
        "functions": [
            {"path": fn.path, "qualname": fn.qualname, "cdg": [e.to_dict() for e in fn.cdg]}
            for fn in functions
        ],
    }


def query_pdg_flows(
    root: Path,
    target: str,
    *,
    variable: Optional[str] = None,
    graph: Optional[CodeGraph] = None,
) -> Dict[str, Any]:
    layer = _pdg_layer(root)
    if layer is None:
        return {"ok": False, "error": _NO_PDG}
    functions = _match_pdg_functions(layer, target)
    if not functions:
        return {"ok": False, "error": f"no PDG for target {target!r}; run `dev map pdg build`"}
    out: List[dict[str, Any]] = []
    for fn in functions:
        flows = fn.reaching_def
        if variable:
            flows = [e for e in flows if e.variable == variable]
        out.append(
            {
                "path": fn.path,
                "qualname": fn.qualname,
                "reaching_def": [e.to_dict() for e in flows],
            }
        )
    return {"ok": True, "target": target, "variable": variable, "functions": out}


# Aliases matching pdg.query public API
explain_taint = explain_pdg_taint
query_controls = query_pdg_controls
query_flows = query_pdg_flows
