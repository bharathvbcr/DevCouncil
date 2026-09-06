"""The Python GraphML exporter duplicates `devmap export`, and does it worse.

`indexing/graph/export.py::export_graphml` and the kernel's
`devmap-query/src/export.rs::export_graphml` render the same artifact into the
same format. That is not an inference: the Rust module's own docstring opens
with "Ported from the GraphML half of `indexing/graph/export.py`", and it
deliberately left the OKF half behind because that half is orchestration.

Two implementations of one format is the setup; the payoff is that they do not
agree, and the survivor is the wrong one. The Rust exporter returns a
`GraphmlReport` carrying `edges_dangling` and `characters_replaced`, and
`devmap --json export` prints both, because — in that CLI's own words — "a
repair nobody is told about is a difference between the graph and its export".
The Python exporter performs the same two repairs, or rather fails to, and
reports nothing.

The tests below are `xfail(strict=True)`, the convention this repository
already uses for a gap it has decided not to paper over: they document the
defect, keep the suite honest, and turn into failures the moment somebody fixes
it. They are not marked because the behaviour is acceptable — they are marked
because the fix belongs to a caller this lane does not own.

**The fix, which needs `src/devcouncil/cli/commands/graph_cmd.py`.** The only
production caller of `export_graphml` is `graph_cmd.py:1784`, inside
`dev graph export --format graphml`. Replace that branch with a call to the
kernel — `devmap export` already accepts `-o -` for stdout and a path
otherwise, and already prints the node/edge/dangling/replaced counts — then
delete `export_graphml` and `_xml` from `export.py`, drop the
`graph_to_graphml = export_graphml` alias at `okf_export.py:34`, and remove
both names from the `_EXPORTS` table in `indexing/graph/__init__.py`. The OKF
half of `export.py` stays: it writes a DevCouncil knowledge bundle for the
planning subsystem, which moves with DevCouncil rather than with the map.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET

import pytest

from devcouncil.indexing.graph.export import export_graphml
from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind


# GraphML declares its namespace as the default, so every element is in it.
# Spelled out rather than reached with ElementTree's `{*}` wildcard: `iter()`
# does not honour that wildcard — only `find`/`findall`/`iterfind` do — so
# `root.iter("{*}edge")` yields nothing at all and every assertion inside the
# loop is skipped. The first draft of this file did exactly that and passed
# while checking nothing.
GRAPHML_NS = "{http://graphml.graphdrawing.org/xmlns}"


def _graph(nodes: list[GraphNode], edges: list[GraphEdge]) -> CodeGraph:
    return CodeGraph(nodes=nodes, edges=edges)


@pytest.mark.xfail(
    strict=True,
    reason=(
        "export.py emits an edge whose endpoints are not declared nodes, which "
        "is invalid GraphML; the kernel drops it and counts edges_dangling. "
        "Needs cli/commands/graph_cmd.py:1784 to move to `devmap export`."
    ),
)
def test_an_edge_to_an_undeclared_node_is_not_emitted() -> None:
    # A dangling endpoint is not hypothetical: `code_graph.json` carries its own
    # `edge_endpoints_without_node` counter precisely because the graph has
    # them, and this exporter writes every edge it is given.
    graph = _graph(
        [GraphNode(id="a.py", kind=NodeKind.FILE, path="a.py", name="a.py")],
        [GraphEdge(source="a.py", target="ghost.py", kind="imports")],
    )
    root = ET.fromstring(export_graphml(graph))
    declared = {node.get("id") for node in root.iter(f"{GRAPHML_NS}node")}
    emitted = list(root.iter(f"{GRAPHML_NS}edge"))
    # The guard that keeps this test from passing by finding nothing to check.
    assert declared, "the export declared no nodes, so this proves nothing"
    assert emitted, "the export emitted no edges, so this proves nothing"
    for edge in emitted:
        assert edge.get("source") in declared and edge.get("target") in declared, (
            "GraphML requires both endpoints of an edge to be declared nodes; "
            "this document names one that is not, and nothing said so"
        )


@pytest.mark.xfail(
    strict=True,
    reason=(
        "export.py escapes the five XML entities but not the C0 control "
        "characters XML 1.0 forbids outright, so the document does not parse. "
        "The kernel replaces them and counts characters_replaced. "
        "Needs cli/commands/graph_cmd.py:1784 to move to `devmap export`."
    ),
)
def test_a_control_character_in_a_symbol_name_still_parses() -> None:
    # A symbol name holds whatever the source file held. No XML escape
    # represents a C0 control, so a document containing one is not repairable
    # by its reader — it simply fails to parse, and the failure surfaces in
    # whatever tool the operator pointed at the export.
    graph = _graph(
        [
            GraphNode(
                id="a.py::we\x08ird",
                kind=NodeKind.SYMBOL,
                path="a.py",
                name="we\x08ird",
            )
        ],
        [],
    )
    xml = export_graphml(graph)
    ET.fromstring(xml)  # raises ParseError on the control byte
