"""Unit coverage for the openCypher subset over the in-memory code graph."""

from __future__ import annotations

from devcouncil.indexing.graph.cypher import _parse_where, run_cypher
from devcouncil.indexing.graph.schema import CodeGraph, Confidence, GraphEdge, GraphNode, NodeKind


def _graph() -> CodeGraph:
    return CodeGraph(
        nodes=[
            GraphNode(id="a.py::foo", kind=NodeKind.FUNCTION, path="a.py", name="foo"),
            GraphNode(id="b.py::bar", kind=NodeKind.FUNCTION, path="pkg/b.py", name="bar"),
            GraphNode(id="c.py::baz", kind=NodeKind.FUNCTION, path="c.py", name="baz"),
        ],
        edges=[
            GraphEdge(
                source="a.py::foo",
                target="b.py::bar",
                kind="calls",
                confidence=Confidence.EXTRACTED,
            ),
            GraphEdge(
                source="a.py::foo",
                target="c.py::baz",
                kind="imports",
                confidence=Confidence.EXTRACTED,
            ),
        ],
    )


def test_parse_where_name_and_path():
    assert _parse_where("") == (None, None)
    assert _parse_where("contains(a.name, 'Foo')") == ("Foo", None)
    assert _parse_where("starts with(b.path, 'pkg/')") == (None, "pkg/")
    assert _parse_where("a.other = 1") == (None, None)


def test_run_cypher_rejects_mutations(tmp_path):
    result = run_cypher(tmp_path, "CREATE (a) RETURN a")
    assert result["ok"] is False
    assert "Mutating" in result["error"]


def test_run_cypher_rejects_unsupported_shape(tmp_path):
    result = run_cypher(tmp_path, "MATCH (a)--(b) RETURN a")
    assert result["ok"] is False
    assert "Unsupported Cypher" in result["error"]


def test_run_cypher_rejects_unknown_rel(tmp_path):
    result = run_cypher(tmp_path, "MATCH (a)-[r:OWNS]->(b) RETURN a,b")
    assert result["ok"] is False
    assert "Unsupported relationship" in result["error"]


def test_run_cypher_no_graph(tmp_path, monkeypatch):
    class Missing:
        def __init__(self, root):
            pass

        def _graph(self):
            raise FileNotFoundError("missing")

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine",
        Missing,
    )
    result = run_cypher(tmp_path, "MATCH (a)-[r:CALLS]->(b) RETURN a,b")
    assert result["ok"] is False
    assert "No committed graph" in result["error"]


def test_run_cypher_calls_with_filters(tmp_path, monkeypatch):
    graph = _graph()

    class FakeEngine:
        def __init__(self, root):
            pass

        def _graph(self):
            return graph

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine",
        FakeEngine,
    )
    result = run_cypher(
        tmp_path,
        "MATCH (a)-[r:CALLS]->(b) WHERE contains(a.name, 'foo') "
        "AND starts with(b.path, 'pkg/') RETURN a,b LIMIT 10",
    )
    assert result["ok"] is True
    assert result["count"] == 1
    assert result["rows"][0]["b_name"] == "bar"


def test_run_cypher_nodes_only(tmp_path, monkeypatch):
    graph = _graph()

    class FakeEngine:
        def __init__(self, root):
            pass

        def _graph(self):
            return graph

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine",
        FakeEngine,
    )
    result = run_cypher(
        tmp_path,
        "MATCH (a) WHERE contains(a.name, 'ba') RETURN a LIMIT 5",
    )
    assert result["ok"] is True
    assert result["count"] == 2
    names = {row["a_name"] for row in result["rows"]}
    assert names == {"bar", "baz"}


def test_run_cypher_imports_relationship(tmp_path, monkeypatch):
    graph = _graph()

    class FakeEngine:
        def __init__(self, root):
            pass

        def _graph(self):
            return graph

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine",
        FakeEngine,
    )
    result = run_cypher(tmp_path, "MATCH (a)-[r:IMPORTS]->(b) RETURN a,b")
    assert result["ok"] is True
    assert result["count"] == 1
    assert result["rows"][0]["rel"] == "imports"
    assert result["rows"][0]["b_name"] == "baz"


def test_run_cypher_rejects_delete(tmp_path):
    result = run_cypher(tmp_path, "MATCH (a) DELETE a RETURN a")
    assert result["ok"] is False
    assert "Mutating" in result["error"]


def _stub_engine(monkeypatch, graph):
    class FakeEngine:
        def __init__(self, root):
            pass

        def _graph(self):
            return graph

    monkeypatch.setattr(
        "devcouncil.codeintel.query.engine.CodeIntelQueryEngine", FakeEngine
    )


def test_run_cypher_bounds_an_unbounded_user_limit(tmp_path, monkeypatch):
    """`LIMIT 999999999` is user text, not a budget the server has to honour."""
    _stub_engine(monkeypatch, _graph())
    result = run_cypher(tmp_path, "MATCH (a) RETURN a LIMIT 999999999")
    assert result["ok"] is True
    assert result["limit_requested"] == 999999999
    assert result["limit_applied"] == 500
    assert result["limit_capped"] is True


def test_run_cypher_keeps_a_limit_inside_the_ceiling(tmp_path, monkeypatch):
    _stub_engine(monkeypatch, _graph())
    result = run_cypher(tmp_path, "MATCH (a) RETURN a LIMIT 10")
    assert result["limit_requested"] == 10
    assert result["limit_applied"] == 10
    assert result["limit_capped"] is False


def test_run_cypher_zero_limit_is_raised_to_one(tmp_path, monkeypatch):
    _stub_engine(monkeypatch, _graph())
    result = run_cypher(tmp_path, "MATCH (a) RETURN a LIMIT 0")
    assert result["limit_applied"] == 1
    assert result["limit_capped"] is True
    assert result["shown"] == 1


def test_run_cypher_node_rows_report_total_beside_the_cap(tmp_path, monkeypatch):
    _stub_engine(monkeypatch, _graph())
    result = run_cypher(tmp_path, "MATCH (a) RETURN a LIMIT 1")
    assert len(result["rows"]) == 1
    assert result["shown"] == 1
    assert result["total"] == 3
    assert result["truncated"] is True
    # `count` keeps its old meaning: rows returned.
    assert result["count"] == 1


def test_run_cypher_edge_rows_report_total_beside_the_cap(tmp_path, monkeypatch):
    _stub_engine(monkeypatch, _graph())
    result = run_cypher(tmp_path, "MATCH (a)-[r:CALLS|IMPORTS]->(b) RETURN a,b LIMIT 1")
    assert result["shown"] == 1
    assert result["total"] == 2
    assert result["truncated"] is True


def test_run_cypher_untruncated_rows_say_so(tmp_path, monkeypatch):
    _stub_engine(monkeypatch, _graph())
    result = run_cypher(tmp_path, "MATCH (a) RETURN a LIMIT 50")
    assert result["shown"] == 3
    assert result["total"] == 3
    assert result["truncated"] is False
