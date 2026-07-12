import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, GraphEdge, DeadCodeEntry, NodeKind, Confidence

runner = CliRunner()


def _setup_graph_env(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    # Construct a mock CodeGraph
    nodes = [
        GraphNode(id="src/a.py", kind=NodeKind.FILE, path="src/a.py", name="a.py"),
        GraphNode(id="src/a.py::func_a", kind=NodeKind.FUNCTION, path="src/a.py", name="func_a"),
        GraphNode(id="src/b.py", kind=NodeKind.FILE, path="src/b.py", name="b.py"),
        GraphNode(id="src/b.py::func_b", kind=NodeKind.FUNCTION, path="src/b.py", name="func_b"),
    ]
    edges = [
        GraphEdge(source="src/a.py::func_a", target="src/b.py::func_b", kind="calls", confidence=Confidence.EXTRACTED),
        GraphEdge(source="src/b.py::func_b", target="src/a.py::func_a", kind="calls", confidence=Confidence.EXTRACTED),  # circular call
    ]
    dead = [
        DeadCodeEntry(id="src/a.py::func_a", path="src/a.py", line=10, kind="function", confidence=Confidence.INFERRED, reason="uncalled"),
    ]
    
    cg = CodeGraph(
        schema_version=2,
        nodes=nodes,
        edges=edges,
        dead_code=dead,
    )
    
    graph_file = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    graph_file.parent.mkdir(parents=True, exist_ok=True)
    graph_file.write_text(cg.model_dump_json(indent=2), encoding="utf-8")
    
    return tmp_path


def test_cli_graph_query(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "query", "src/a.py::func_a"])
    assert res.exit_code == 0
    assert "src/a.py::func_a" in res.output
    
    res_json = runner.invoke(app, ["graph", "query", "src/a.py::func_a", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert len(data["definitions"]) > 0


def test_cli_graph_trace(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "trace", "src/a.py::func_a", "src/b.py::func_b"])
    assert res.exit_code == 0
    assert "src/a.py::func_a → src/b.py::func_b" in res.output
    
    res_json = runner.invoke(app, ["graph", "trace", "src/a.py::func_a", "src/b.py::func_b", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["found"] is True


def test_cli_graph_dead(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "dead"])
    assert res.exit_code == 0
    assert "src/a.py::func_a" in res.output
    assert "uncalled" in res.output
    
    res_json = runner.invoke(app, ["graph", "dead", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert len(data) == 1
    assert data[0]["id"] == "src/a.py::func_a"


def test_cli_graph_check(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "check"])
    assert res.exit_code == 0
    assert "God nodes" in res.output
    assert "Circular imports" in res.output
    
    res_json = runner.invoke(app, ["graph", "check", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert "god_nodes" in data
    assert "circular_imports" in data


def test_cli_graph_process(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    # Query with entry roots
    res = runner.invoke(app, ["graph", "process"])
    assert res.exit_code == 0
    
    res_json = runner.invoke(app, ["graph", "process", "--json"])
    assert res_json.exit_code == 0


def test_cli_graph_impact(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "impact", "src/a.py"])
    assert res.exit_code == 0
    assert "src/a.py" in res.output
    
    res_json = runner.invoke(app, ["graph", "impact", "src/a.py", "--json"])
    assert res_json.exit_code == 0


def test_cli_graph_html(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "html"])
    assert res.exit_code == 0, f"res.output: {res.output}"
    assert "Wrote" in res.output
    # Check if the file exists in the graph directory
    assert (tmp_path / ".devcouncil" / "graph" / "graph.html").exists()


def test_cli_graph_export(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["graph", "export", "--format", "graphml"])
    assert res.exit_code == 0
    assert "graph" in res.output
    
    res_okf = runner.invoke(app, ["graph", "export", "--format", "okf", "-o", "okf-out"])
    assert res_okf.exit_code == 0
    assert "Wrote OKF bundle" in res_okf.output
