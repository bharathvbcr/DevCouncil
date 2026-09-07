import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, GraphEdge, DeadCodeEntry, NodeKind, Confidence

runner = CliRunner()


def _setup_graph_env(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.chdir(tmp_path)
    # No initial map: `dev init` now builds a kernel store from whatever files
    # exist (the agent guides it writes are enough for a non-empty generation),
    # and the kernel then answers ahead of the hand-built Python graph below.
    # These tests exercise the CLI rendering of the compatibility read path, so
    # they must start with no kernel store at all.
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)

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
    
    # Persist via canonical SQLite path — writing JSON alone is ignored when a
    # store already exists from ``dev init``.
    from devcouncil.indexing.graph.build import write_code_graph

    write_code_graph(tmp_path, cg)
    
    return tmp_path


def test_cli_graph_query_refuses_a_python_only_store(tmp_path, monkeypatch):
    """`dev map query` has no Python engine to fall back to, and says so.

    Rewritten: this asserted that the command rendered a hand-built graph
    written into the *Python* store — the compatibility read path
    `_setup_graph_env` builds. That path is retired: `query_symbol` and
    `trace_path` are gone and the kernel is the only engine, so a tree with a
    Python store and no devmap generation gets an error naming the kernel.

    The rendering these used to cover is covered against the kernel seam in
    `test_graph_cmd_command.py` (`test_graph_query_json`,
    `test_graph_query_human_with_defs`).
    """
    _setup_graph_env(tmp_path, monkeypatch)

    res = runner.invoke(app, ["map", "query", "src/a.py::func_a"])
    assert res.exit_code != 0
    assert "dev map" in res.output or "devmap" in res.output, res.output


def test_cli_graph_trace_refuses_a_python_only_store(tmp_path, monkeypatch):
    """Same, and here the deleted engine was wrong rather than merely slow.

    The Python tracer walked the graph *undirected*, so it reported paths the
    kernel's directional walk finds no evidence for.
    """
    _setup_graph_env(tmp_path, monkeypatch)

    res = runner.invoke(app, ["map", "trace", "src/a.py::func_a", "src/b.py::func_b"])
    assert res.exit_code != 0
    assert "dev map" in res.output or "devmap" in res.output, res.output


def test_cli_graph_dead(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["map", "dead"])
    assert res.exit_code == 0
    assert "src/a.py::func_a" in res.output
    assert "uncalled" in res.output
    
    res_json = runner.invoke(app, ["map", "dead", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.stdout)
    assert len(data["dead_code"]) == 1
    assert data["dead_code"][0]["id"] == "src/a.py::func_a"


def test_cli_graph_check(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["map", "check"])
    assert res.exit_code == 0
    assert "God nodes" in res.output
    assert "Circular imports" in res.output
    
    res_json = runner.invoke(app, ["map", "check", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.stdout)
    assert "god_nodes" in data
    assert "circular_imports" in data


def test_cli_graph_process(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    # Query with entry roots
    res = runner.invoke(app, ["map", "process"])
    assert res.exit_code == 0
    
    res_json = runner.invoke(app, ["map", "process", "--json"])
    assert res_json.exit_code == 0


def test_cli_graph_impact_without_a_kernel_store_is_red(tmp_path, monkeypatch):
    """Rewritten, for the reason the GraphML sibling below states.

    This environment holds a hand-built Python graph and no kernel store, and
    `dev map impact` used to answer from that graph — running the inbound walk
    in Python and labelling every symbol a depth-3 walk reached `depth: 1,
    confidence: "extracted"`. The bands are the kernel's now, so no kernel is an
    error naming it rather than a second engine's answer.
    """
    _setup_graph_env(tmp_path, monkeypatch)

    res = runner.invoke(app, ["map", "impact", "src/a.py"])
    assert res.exit_code == 1, res.output
    assert "devmap" in res.output.lower()

    res_json = runner.invoke(app, ["map", "impact", "src/a.py", "--json"])
    assert res_json.exit_code == 1


def test_cli_graph_impact_from_the_kernel(tmp_path, monkeypatch):
    """The real producer, end to end: sources → `devmap build` → `dev map impact`."""
    import subprocess

    from tests.unit.graph_fixtures import kernel_graph

    monkeypatch.chdir(tmp_path)
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "a.py").write_text(
        "from pkg.b import func_b\n\ndef func_a():\n    return func_b()\n",
        encoding="utf-8",
    )
    (tmp_path / "pkg" / "b.py").write_text("def func_b():\n    return 1\n", encoding="utf-8")
    for args in (
        ["init"],
        ["add", "-A"],
        ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
    ):
        subprocess.run(["git", *args], cwd=tmp_path, check=True, capture_output=True)
    kernel_graph(tmp_path)  # builds the store, or skips when no kernel is built

    res = runner.invoke(app, ["map", "impact", "pkg/b.py"])
    assert res.exit_code == 0, res.output
    assert "pkg/b.py" in res.output
    assert "depth 1" in res.output

    res_json = runner.invoke(app, ["map", "impact", "pkg/b.py", "--json"])
    assert res_json.exit_code == 0, res_json.output
    payload = json.loads(res_json.stdout)
    assert payload["source"] == "devmap"
    entry = payload["paths"][0]
    assert entry["blast"]["seeds"], entry
    assert entry["blast"]["total_impacted"] >= 1, entry


def test_cli_graph_html(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["map", "graph-html"])
    assert res.exit_code == 0, f"res.output: {res.output}"
    assert "Wrote" in res.output
    # Check if the file exists in the graph directory
    assert (tmp_path / ".devcouncil" / "graph" / "graph.html").exists()


def test_cli_graph_export_okf_from_the_compatibility_graph(tmp_path, monkeypatch):
    _setup_graph_env(tmp_path, monkeypatch)

    res_okf = runner.invoke(app, ["map", "export", "--format", "okf", "-o", "okf-out"])
    assert res_okf.exit_code == 0
    assert "Wrote OKF bundle" in res_okf.output


def test_cli_graph_export_graphml_without_a_kernel_store_is_red(tmp_path, monkeypatch):
    """GraphML comes from the kernel and only from the kernel.

    This environment holds a hand-built Python graph and no kernel store. The
    Python exporter that used to answer here was a duplicate of the kernel's
    (deleted 2026-09-06, see `devmap_engine.export_graphml`); answering from it
    now would be the silent second engine the map's own comment calls "a
    defect, not a mode". So: exit 1, the kernel named, no GraphML on stdout.
    """
    _setup_graph_env(tmp_path, monkeypatch)

    res = runner.invoke(app, ["map", "export", "--format", "graphml"])
    assert res.exit_code == 1, res.output
    assert "<graphml" not in res.output
    assert "devmap" in res.output.lower()


def test_cli_graph_export_graphml_from_the_kernel(tmp_path, monkeypatch):
    """The real producer, end to end: sources → `devmap build` → `dev map export`."""
    import subprocess

    from tests.unit.graph_fixtures import kernel_graph

    monkeypatch.chdir(tmp_path)
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "a.py").write_text("from pkg.b import func_b\n\ndef func_a():\n    return func_b()\n", encoding="utf-8")
    (tmp_path / "pkg" / "b.py").write_text("def func_b():\n    return 1\n", encoding="utf-8")
    for args in (["init"], ["add", "-A"], ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]):
        subprocess.run(["git", *args], cwd=tmp_path, check=True, capture_output=True)
    kernel_graph(tmp_path)  # builds the store, or skips when no kernel is built

    out = tmp_path / "out" / "g.graphml"
    res = runner.invoke(app, ["map", "export", "--format", "graphml", "-o", str(out)])
    assert res.exit_code == 0, res.output
    text = out.read_text(encoding="utf-8")
    assert text.startswith("<?xml")
    assert 'attr.name="dead"' in text and 'attr.name="community"' in text
    assert "pkg/a.py::func_a" in text and "pkg/b.py::func_b" in text
    # The kernel's counts reach the operator; Rich wraps at 80 columns.
    assert "nodes" in " ".join(res.output.split())

    res_stdout = runner.invoke(app, ["map", "export", "--format", "graphml"])
    assert res_stdout.exit_code == 0, res_stdout.output
    assert res_stdout.output.startswith("<?xml")


# --- thin `dev graph` alias suite (dual-registered map app) ---------------------


def test_graph_alias_query_and_html(tmp_path, monkeypatch):
    """Compatibility: `dev graph …` remains dual-registered with `dev map …`."""
    _setup_graph_env(tmp_path, monkeypatch)

    # The alias reaches the *same* command, so it reaches the same refusal now
    # that `dev map query` has no Python engine behind it. What is under test
    # here is the dual registration, not the answer.
    res = runner.invoke(app, ["graph", "query", "src/a.py::func_a"])
    assert res.exit_code != 0
    assert "No such command" not in res.output, res.output
    assert "dev map" in res.output or "devmap" in res.output, res.output

    res_html = runner.invoke(app, ["graph", "html"])
    assert res_html.exit_code == 0
    assert (tmp_path / ".devcouncil" / "graph" / "graph.html").exists()


def test_graph_alias_demo(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    res = runner.invoke(app, ["graph", "demo", "--json", "--project-root", str(tmp_path)])
    assert res.exit_code == 0
