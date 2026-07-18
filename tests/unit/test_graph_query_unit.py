"""Unit tests for indexing.graph.query helpers (stable, deterministic)."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from devcouncil.indexing.graph.query import (
    _match_nodes,
    explain_pdg_taint,
    query_pdg_controls,
    query_pdg_flows,
    query_symbol,
    symbol_has_non_test_inbound,
    trace_path,
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


def test_query_symbol_missing_graph(tmp_path: Path):
    result = query_symbol(tmp_path, "foo", graph=None)
    assert "error" in result
    assert result["query"] == "foo"


def test_query_symbol_no_matches(tmp_path: Path):
    result = query_symbol(tmp_path, "missing", graph=_graph())
    assert result["matches"] == []
    assert result["definitions"] == []


def test_query_symbol_match_by_suffix_and_edges(tmp_path: Path):
    result = query_symbol(tmp_path, "foo", graph=_graph())
    assert result["matches"] >= 1
    defs = result["definitions"]
    assert defs
    assert "pkg/b.py::bar" in defs[0]["callers"]


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


def test_trace_path_missing_graph_and_endpoints(tmp_path: Path):
    assert "error" in trace_path(tmp_path, "a", "b", graph=None)
    missing = trace_path(tmp_path, "zzz", "yyy", graph=_graph())
    assert missing["found"] is False
    assert missing["reason"] == "endpoint not found"


def test_trace_path_found_and_not_found(tmp_path: Path):
    found = trace_path(tmp_path, "pkg/b.py", "pkg/a.py", graph=_graph())
    assert found["found"] is True
    assert found["path"]
    orphan = CodeGraph(
        nodes=[
            GraphNode(id="alpha", kind=NodeKind.FILE, path="alpha.py", name="alpha"),
            GraphNode(id="beta", kind=NodeKind.FILE, path="beta.py", name="beta"),
        ],
        edges=[],
    )
    assert trace_path(tmp_path, "alpha", "beta", graph=orphan, max_depth=0)["found"] is False


def test_explain_pdg_taint_no_graph(tmp_path: Path):
    result = explain_pdg_taint(tmp_path, graph=None)
    assert result["ok"] is False


def test_explain_pdg_taint_from_meta_and_filters(tmp_path: Path):
    g = CodeGraph(
        nodes=[],
        edges=[],
        meta={
            "pdg": {
                "taint_findings": [
                    {
                        "path": "a.py",
                        "function": "f",
                        "category": "sql",
                        "source_line": 1,
                        "sink_line": 2,
                        "variable": "q",
                        "source_expr": "input",
                        "sink_expr": "execute",
                    },
                    {
                        "path": "b.py",
                        "function": "g",
                        "category": "cmd",
                        "source_line": 1,
                        "sink_line": 2,
                        "variable": "c",
                        "source_expr": "argv",
                        "sink_expr": "system",
                    },
                ]
            }
        },
    )
    with patch("devcouncil.indexing.graph.build.load_pdg_layer", return_value=None):
        all_findings = explain_pdg_taint(tmp_path, graph=g)
        assert all_findings["ok"] is True
        assert all_findings["count"] == 2
        filtered = explain_pdg_taint(tmp_path, graph=g, path="a.py", category="sql")
        assert filtered["count"] == 1


def test_query_pdg_controls_and_flows_no_graph(tmp_path: Path):
    assert query_pdg_controls(tmp_path, "f", graph=None)["ok"] is False
    assert query_pdg_flows(tmp_path, "f", graph=None)["ok"] is False


def test_query_pdg_controls_and_flows_with_mock_functions(tmp_path: Path):
    g = _graph()
    cdg_edge = SimpleNamespace(to_dict=lambda: {"kind": "cdg"})
    rd_edge = SimpleNamespace(variable="x", to_dict=lambda: {"variable": "x"})
    fn = SimpleNamespace(
        path="pkg/a.py",
        qualname="pkg.a.foo",
        cdg=[cdg_edge],
        reaching_def=[rd_edge, SimpleNamespace(variable="y", to_dict=lambda: {"variable": "y"})],
    )
    with patch("devcouncil.indexing.graph.query._match_pdg_functions", return_value=[fn]):
        controls = query_pdg_controls(tmp_path, "foo", graph=g)
        assert controls["ok"] is True
        assert controls["functions"][0]["cdg"] == [{"kind": "cdg"}]
        flows = query_pdg_flows(tmp_path, "foo", variable="x", graph=g)
        assert flows["ok"] is True
        assert len(flows["functions"][0]["reaching_def"]) == 1

    with patch("devcouncil.indexing.graph.query._match_pdg_functions", return_value=[]):
        assert query_pdg_controls(tmp_path, "missing", graph=g)["ok"] is False
        assert query_pdg_flows(tmp_path, "missing", graph=g)["ok"] is False


def test_load_file_pdg_and_match_functions(tmp_path: Path):
    from devcouncil.indexing.graph.pdg.schema import FilePDG, FunctionPDG
    from devcouncil.indexing.graph.query import _load_file_pdg_from_store, _match_pdg_functions

    assert _load_file_pdg_from_store(tmp_path, "a.py") is None

    file_pdg = FilePDG(
        path="pkg/a.py",
        language="python",
        functions=[
            FunctionPDG(path="pkg/a.py", qualname="pkg.a.foo", start_line=1, end_line=5)
        ],
    )
    store = MagicMock()
    store.analysis_shards.return_value = {"pkg/a.py": {"pdg": file_pdg.to_dict()}}
    service = MagicMock(store=store)
    with patch("devcouncil.codeintel.get_codeintel_service", return_value=service):
        loaded = _load_file_pdg_from_store(tmp_path, "pkg/a.py")
        assert loaded is not None
        hits = _match_pdg_functions(tmp_path, _graph(), "pkg/a.py")
        assert hits
        hits2 = _match_pdg_functions(tmp_path, _graph(), "foo")
        assert hits2
