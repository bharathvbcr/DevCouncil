"""Unit tests for indexing.graph.query helpers (stable, deterministic)."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from devcouncil.indexing.graph.query import (
    _match_nodes,
    explain_pdg_taint,
    query_pdg_controls,
    query_pdg_flows,
    symbol_has_non_test_inbound,
)
from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind


def _graph() -> CodeGraph:
    nodes = [
        GraphNode(id="pkg/a.py", kind=NodeKind.FILE, path="pkg/a.py", name="a.py"),
        GraphNode(id="pkg/a.py::foo", kind=NodeKind.FUNCTION, path="pkg/a.py", name="foo", line=1),
        GraphNode(id="pkg/b.py", kind=NodeKind.FILE, path="pkg/b.py", name="b.py"),
        GraphNode(id="pkg/b.py::bar", kind=NodeKind.FUNCTION, path="pkg/b.py", name="bar", line=2),
        GraphNode(id="tests/t.py::test_foo", kind=NodeKind.FUNCTION, path="tests/t.py", name="test_foo"),
    ]
    edges = [
        GraphEdge(source="pkg/b.py::bar", target="pkg/a.py::foo", kind="calls"),
        GraphEdge(source="pkg/b.py", target="pkg/a.py", kind="imports"),
        GraphEdge(source="pkg/a.py", target="pkg/a.py::foo", kind="contains"),
        GraphEdge(source="tests/t.py::test_foo", target="pkg/a.py::foo", kind="calls"),
    ]
    return CodeGraph(nodes=nodes, edges=edges)


def test_match_nodes_path_and_id_suffix():
    g = _graph()
    assert _match_nodes(g, "pkg/a.py")
    assert _match_nodes(g, r"pkg\a.py")
    assert any(n.name == "foo" for n in _match_nodes(g, "foo"))


def test_symbol_has_non_test_inbound_paths(tmp_path: Path):
    assert symbol_has_non_test_inbound(tmp_path, "missing.py", "x", graph=None) is False
    assert symbol_has_non_test_inbound(tmp_path, "nope.py", "foo", graph=_graph()) is False
    assert symbol_has_non_test_inbound(tmp_path, "pkg/a.py", "foo", graph=_graph()) is True


def test_symbol_has_non_test_inbound_ignores_test_only(tmp_path: Path):
    g = CodeGraph(
        nodes=[
            GraphNode(id="pkg/a.py::foo", kind=NodeKind.FUNCTION, path="pkg/a.py", name="foo"),
            GraphNode(id="tests/t.py::test_foo", kind=NodeKind.FUNCTION, path="tests/t.py", name="test_foo"),
        ],
        edges=[GraphEdge(source="tests/t.py::test_foo", target="pkg/a.py::foo", kind="calls")],
    )
    assert symbol_has_non_test_inbound(tmp_path, "pkg/a.py", "foo", graph=g) is False


def test_explain_pdg_taint_no_graph(tmp_path: Path):
    result = explain_pdg_taint(tmp_path, graph=None)
    assert result["ok"] is False


def _sidecar(tmp_path: Path, findings) -> None:
    """Build the PDG sidecar the query surfaces now read.

    The layer used to arrive as ``graph.meta["pdg"]``, so these tests handed
    `explain_pdg_taint` a `CodeGraph` with a hand-written meta blob. The layer
    lives in `.devcouncil/graph/pdg.json` now, written by `dev map --pdg` /
    `dev map pdg build`, so the fixture writes that file instead. Going through
    `write_pdg_layer` rather than hand-rolling JSON keeps the fixture honest
    about the on-disk shape.
    """
    from devcouncil.indexing.graph.build import write_pdg_layer
    from devcouncil.indexing.graph.pdg.schema import FilePDG, FunctionPDG, PDGLayer

    layer = PDGLayer()
    for finding in findings:
        file_pdg = layer.files.setdefault(
            finding.path, FilePDG(path=finding.path, language="python", functions=[])
        )
        file_pdg.functions.append(
            FunctionPDG(
                path=finding.path,
                qualname=finding.function,
                start_line=finding.source_line,
                end_line=finding.sink_line,
                taint=[finding],
            )
        )
    write_pdg_layer(tmp_path, layer)


def test_explain_pdg_taint_from_the_sidecar_and_filters(tmp_path: Path):
    from devcouncil.indexing.graph.pdg.schema import TaintFinding

    _sidecar(
        tmp_path,
        [
            TaintFinding(
                path="a.py", function="f", category="sql", source_line=1, sink_line=2,
                variable="q", source_expr="input", sink_expr="execute",
            ),
            TaintFinding(
                path="b.py", function="g", category="cmd", source_line=1, sink_line=2,
                variable="c", source_expr="argv", sink_expr="system",
            ),
        ],
    )
    all_findings = explain_pdg_taint(tmp_path)
    assert all_findings["ok"] is True
    assert all_findings["count"] == 2
    filtered = explain_pdg_taint(tmp_path, path="a.py", category="sql")
    assert filtered["count"] == 1


def test_explain_pdg_taint_is_complete_not_the_first_five_hundred(tmp_path: Path):
    """The answer to "what reaches a sink" must not be a silently capped sample.

    The findings used to come from ``graph.meta["pdg"]``, and ``PDGLayer.to_meta``
    trims ``taint_findings`` to ``findings[:500]`` while ``stats.taint_count``
    keeps the true total — so a repository with more than 500 findings had the
    first 500 published as the whole answer, with nothing in the payload saying
    so. The sidecar carries every function's findings.
    """
    from devcouncil.indexing.graph.pdg.schema import TaintFinding

    findings = [
        TaintFinding(
            path=f"m{i:04d}.py", function=f"f{i}", category="sql", source_line=1,
            sink_line=2, variable="q", source_expr="input", sink_expr="execute",
        )
        for i in range(600)
    ]
    _sidecar(tmp_path, findings)
    assert explain_pdg_taint(tmp_path)["count"] == 600


def test_query_pdg_controls_and_flows_no_graph(tmp_path: Path):
    assert query_pdg_controls(tmp_path, "f", graph=None)["ok"] is False
    assert query_pdg_flows(tmp_path, "f", graph=None)["ok"] is False


def test_query_pdg_controls_and_flows_with_mock_functions(tmp_path: Path):
    cdg_edge = SimpleNamespace(to_dict=lambda: {"kind": "cdg"})
    rd_edge = SimpleNamespace(variable="x", to_dict=lambda: {"variable": "x"})
    fn = SimpleNamespace(
        path="pkg/a.py",
        qualname="pkg.a.foo",
        cdg=[cdg_edge],
        reaching_def=[rd_edge, SimpleNamespace(variable="y", to_dict=lambda: {"variable": "y"})],
    )
    # The layer is what these read now, not a `CodeGraph`: `_pdg_layer` stands in
    # for the sidecar so the rendering assertions stay on `_match_pdg_functions`.
    layer = SimpleNamespace(files={"pkg/a.py": SimpleNamespace(functions=[fn])})
    with patch("devcouncil.indexing.graph.query._pdg_layer", return_value=layer):
        with patch("devcouncil.indexing.graph.query._match_pdg_functions", return_value=[fn]):
            controls = query_pdg_controls(tmp_path, "foo")
            assert controls["ok"] is True
            assert controls["functions"][0]["cdg"] == [{"kind": "cdg"}]
            flows = query_pdg_flows(tmp_path, "foo", variable="x")
            assert flows["ok"] is True
            assert len(flows["functions"][0]["reaching_def"]) == 1

        with patch("devcouncil.indexing.graph.query._match_pdg_functions", return_value=[]):
            assert query_pdg_controls(tmp_path, "missing")["ok"] is False
            assert query_pdg_flows(tmp_path, "missing")["ok"] is False


def test_match_pdg_functions_resolves_a_path_or_a_bare_name(tmp_path: Path):
    """Matching is the layer's own job now, with no graph involved.

    This used to load one file's PDG out of the Python store's analysis shards
    (`_load_file_pdg_from_store`, gone) and resolve a bare name to files by
    scanning `graph.nodes` — a whole-graph read to answer a question the layer's
    own `qualname`s answer, since a function with no PDG entry cannot be in the
    result either way.
    """
    from devcouncil.indexing.graph.build import read_pdg_layer_file, write_pdg_layer
    from devcouncil.indexing.graph.pdg.schema import FilePDG, FunctionPDG, PDGLayer
    from devcouncil.indexing.graph.query import _match_pdg_functions

    assert read_pdg_layer_file(tmp_path) is None, "no sidecar is None, not an empty layer"

    layer = PDGLayer()
    layer.files["pkg/a.py"] = FilePDG(
        path="pkg/a.py",
        language="python",
        functions=[
            FunctionPDG(path="pkg/a.py", qualname="pkg.a.foo", start_line=1, end_line=5)
        ],
    )
    write_pdg_layer(tmp_path, layer)

    round_tripped = read_pdg_layer_file(tmp_path)
    assert round_tripped is not None
    assert _match_pdg_functions(round_tripped, "pkg/a.py"), "a path must match"
    assert _match_pdg_functions(round_tripped, "foo"), "a bare name must match"
    assert not _match_pdg_functions(round_tripped, "pkg/absent.py")
