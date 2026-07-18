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
