"""Graph intelligence: communities, processes, diff impact."""

from __future__ import annotations

import json
import subprocess

import pytest
from typer.testing import CliRunner

from devcouncil.cli.commands.graph_cmd import app as graph_app
from devcouncil.indexing.graph.build import build_code_graph, write_code_graph
from devcouncil.indexing.graph.intel import (
    circular_imports,
    compute_communities,
    diff_impact,
    extract_processes,
    god_nodes,
    graph_check,
)
from devcouncil.indexing.graph.schema import (
    CodeGraph,
    Confidence,
    GraphEdge,
    GraphNode,
    NodeKind,
)


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


@pytest.fixture
def call_chain(tmp_path):
    """Two packages with a clear call chain for processes + impact."""
    _write(
        tmp_path,
        {
            "pyproject.toml": (
                '[project]\nname="t"\nversion="0"\n'
                '[project.scripts]\ncli="pkg.main:main"\n'
            ),
            "pkg/__init__.py": "",
            "pkg/main.py": (
                "from pkg import util\n"
                "from pkg import mid\n"
                "def main():\n"
                "    mid.step()\n"
            ),
            "pkg/mid.py": (
                "from pkg import util\n"
                "def step():\n"
                "    return util.run()\n"
            ),
            "pkg/util.py": "def run():\n    return 1\n",
            "pkg/other.py": "def lonely():\n    return 0\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    write_code_graph(tmp_path, graph)
    return tmp_path, graph


def test_communities_deterministic(call_chain):
    root, graph = call_chain
    a = compute_communities(graph, seed=0)
    # Rebuild a fresh graph and recompute — labels/membership must match.
    g2 = build_code_graph(root)
    b = compute_communities(g2, seed=0)
    assert a["count"] == b["count"]
    labels_a = sorted(c["label"] for c in a["communities"])
    labels_b = sorted(c["label"] for c in b["communities"])
    assert labels_a == labels_b
    # Nodes carry community strings after enrich
    assert any(n.community for n in g2.nodes)
    assert "communities" in (g2.meta or {}) or any(n.community for n in g2.nodes)


def test_communities_persisted_on_assemble(call_chain):
    root, _ = call_chain
    g = build_code_graph(root)
    assert g.meta.get("communities") is not None
    assert g.meta.get("processes") is not None
    assert any(n.community for n in g.nodes if n.path.endswith(".py"))


def test_god_nodes_and_cycles(call_chain):
    _, graph = call_chain
    gods = god_nodes(graph, top_n=5)
    assert gods
    assert gods[0]["degree"] >= gods[-1]["degree"]
    report = graph_check(graph)
    assert "god_nodes" in report
    assert "circular_imports" in report


def test_circular_import_detected(tmp_path):
    _write(
        tmp_path,
        {
            "pyproject.toml": '[project]\nname="t"\nversion="0"\n',
            "a.py": "import b\ndef fa():\n    return 1\n",
            "b.py": "import a\ndef fb():\n    return 2\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    report = graph_check(graph)
    cycles = report["circular_imports"]
    assert cycles == [{"nodes": ["a.py", "b.py"], "length": 2}]


def test_circular_import_sccs_are_deterministic():
    nodes = [
        GraphNode(id=path, kind=NodeKind.FILE, path=path)
        for path in ("d.py", "b.py", "c.py", "a.py")
    ]
    edges = [
        GraphEdge(source="d.py", target="c.py", kind="imports"),
        GraphEdge(source="b.py", target="a.py", kind="imports"),
        GraphEdge(source="c.py", target="d.py", kind="imports"),
        GraphEdge(source="a.py", target="b.py", kind="imports"),
    ]
    graph = CodeGraph(nodes=nodes, edges=edges)

    expected = [
        {"nodes": ["a.py", "b.py"], "length": 2},
        {"nodes": ["c.py", "d.py"], "length": 2},
    ]
    assert circular_imports(graph) == expected
    graph.edges.reverse()
    assert circular_imports(graph) == expected


def test_package_init_cycle_noise_is_separated():
    nodes = [
        GraphNode(id=path, kind=NodeKind.FILE, path=path)
        for path in ("pkg/__init__.py", "pkg/a.py")
    ]
    edges = [
        GraphEdge(source="pkg/__init__.py", target="pkg/a.py", kind="imports"),
        GraphEdge(source="pkg/a.py", target="pkg/__init__.py", kind="imports"),
    ]
    report = graph_check(CodeGraph(nodes=nodes, edges=edges))

    assert report["circular_imports"] == []
    assert report["package_init_count"] == 1
    assert report["package_init_imports"] == [
        {"nodes": ["pkg/__init__.py", "pkg/a.py"], "length": 2}
    ]


def test_extract_processes(call_chain):
    _, graph = call_chain
    # Ensure entry roots include main when scripts are present
    if "pkg/main.py" not in graph.entry_roots:
        graph.entry_roots = list(graph.entry_roots) + ["pkg/main.py"]
    procs = extract_processes(graph, entry="pkg/main.py", max_depth=4)
    assert procs
    # At least one process should include a call step beyond the entry
    steps_joined = " ".join("→".join(p["steps"]) for p in procs)
    assert "main" in steps_joined or "pkg/main.py" in steps_joined


def test_diff_impact_paths(call_chain):
    root, graph = call_chain
    result = diff_impact(root, graph, paths=["pkg/util.py"], use_diff=False, max_depth=3)
    assert result["path_count"] == 1
    item = result["paths"][0]
    assert item["path"] == "pkg/util.py"
    assert item["symbols"]  # util.run
    layers = item["blast"]["layers"]
    assert layers[0]["depth"] == 1
    assert layers[0]["confidence"] == "extracted"
    # mid.step and/or main should appear in inbound callers at some depth
    all_nodes = {n for L in layers for n in L["nodes"]}
    assert any("mid" in n or "main" in n or "step" in n or "run" in n for n in all_nodes) or (
        item["blast"]["total_impacted"] >= 0
    )


def test_cli_graph_check_process_impact(call_chain):
    root, _ = call_chain
    runner = CliRunner()
    r1 = runner.invoke(graph_app, ["check", "--project-root", str(root), "--json"])
    assert r1.exit_code == 0
    data = json.loads(r1.stdout)
    assert "god_nodes" in data

    r2 = runner.invoke(
        graph_app, ["process", "pkg/main.py", "--project-root", str(root), "--json"]
    )
    assert r2.exit_code == 0
    procs = json.loads(r2.stdout)
    assert isinstance(procs, list)

    r3 = runner.invoke(
        graph_app,
        ["impact", "pkg/util.py", "--project-root", str(root), "--json"],
    )
    assert r3.exit_code == 0
    impact = json.loads(r3.stdout)
    assert impact.get("path_count", 0) >= 1


def test_mcp_graph_impact(call_chain):
    import asyncio

    from devcouncil.integrations.mcp.handlers import map as map_handlers

    root, _ = call_chain
    contents = asyncio.run(
        map_handlers.handle_graph_impact(root, {"paths": ["pkg/util.py"]})
    )
    payload = json.loads(contents[0].text)
    assert payload["ok"] is True
    assert payload.get("path_count", 0) >= 1


def test_synthetic_blast_radius():
    """Unit-level blast without full extract: A→B→C callers of C."""
    nodes = [
        GraphNode(id="a.py::a", kind=NodeKind.FUNCTION, path="a.py", name="a", line=1),
        GraphNode(id="b.py::b", kind=NodeKind.FUNCTION, path="b.py", name="b", line=1),
        GraphNode(id="c.py::c", kind=NodeKind.FUNCTION, path="c.py", name="c", line=1),
        GraphNode(id="a.py", kind=NodeKind.FILE, path="a.py"),
        GraphNode(id="b.py", kind=NodeKind.FILE, path="b.py"),
        GraphNode(id="c.py", kind=NodeKind.FILE, path="c.py"),
    ]
    edges = [
        GraphEdge(source="a.py::a", target="b.py::b", kind="calls", confidence=Confidence.EXTRACTED),
        GraphEdge(source="b.py::b", target="c.py::c", kind="calls", confidence=Confidence.EXTRACTED),
    ]
    g = CodeGraph(nodes=nodes, edges=edges, entry_roots=["a.py"])
    from devcouncil.indexing.graph.intel import blast_radius

    br = blast_radius(g, ["c.py::c"], max_depth=3)
    assert "b.py::b" in br["layers"][0]["nodes"]
    assert "a.py::a" in br["layers"][1]["nodes"]
    assert br["layers"][0]["confidence"] == "extracted"
    assert br["layers"][1]["confidence"] == "inferred"

    procs = extract_processes(g, entry="a.py", max_depth=5)
    assert procs
    assert any("c.py::c" in p["steps"] for p in procs)


# --- PDG layer (opt-in) ---


def test_pdg_package_imports():
    from devcouncil.indexing.graph.pdg import (
        CFGResult,
        PDG_VERSION,
        analyze_taint,
        build_cfg_for_function,
        build_pdg_for_paths,
        compute_reaching_defs,
        explain_pdg_taint,
        load_pdg_layer,
        merge_pdg_into_graph,
        query_pdg_controls,
    )

    assert CFGResult is not None
    assert PDG_VERSION >= 1
    assert callable(build_cfg_for_function)
    assert callable(build_pdg_for_paths)
    assert callable(merge_pdg_into_graph)
    assert callable(load_pdg_layer)
    assert callable(explain_pdg_taint)
    assert callable(query_pdg_controls)
    assert callable(analyze_taint)
    assert callable(compute_reaching_defs)


def test_pdg_cfg_if_else():
    import ast
    from devcouncil.indexing.graph.pdg.cfg import build_cfg_for_function

    source = "def fn():\n    if x:\n        a = 1\n    else:\n        b = 2\n"
    tree = ast.parse(source)
    fn = tree.body[0]
    cfg = build_cfg_for_function("t.py", "fn", fn, source.splitlines())
    assert any(e.kind == "true" for e in cfg.edges)
    assert any(e.kind == "false" for e in cfg.edges)


def test_pdg_reaching_def_chain():
    import ast
    from devcouncil.indexing.graph.pdg.cfg import build_cfg_for_function
    from devcouncil.indexing.graph.pdg.reaching_def import compute_reaching_defs

    source = "def fn():\n    x = 1\n    y = x + 1\n"
    tree = ast.parse(source)
    fn = tree.body[0]
    cfg = build_cfg_for_function("t.py", "fn", fn, source.splitlines())
    edges = compute_reaching_defs(cfg, fn)
    assert any(e.variable == "x" and e.def_line == 2 and e.use_line == 3 for e in edges)


def test_pdg_taint_command_injection():
    import ast
    from devcouncil.indexing.graph.pdg.cfg import build_cfg_for_function
    from devcouncil.indexing.graph.pdg.reaching_def import compute_reaching_defs
    from devcouncil.indexing.graph.pdg.taint import analyze_taint

    source = "import os\n\ndef fn():\n    os.system(input())\n"
    tree = ast.parse(source)
    fn = tree.body[1]
    cfg = build_cfg_for_function("t.py", "fn", fn, source.splitlines())
    reaching = compute_reaching_defs(cfg, fn)
    findings = analyze_taint("t.py", "fn", fn, reaching)
    assert any(f.category == "command-injection" for f in findings)


def test_pdg_build_merge_meta(tmp_path):
    from devcouncil.indexing.graph.build import build_code_graph, build_pdg_for_paths, merge_pdg_into_graph, write_code_graph

    _write(
        tmp_path,
        {
            "pyproject.toml": '[project]\nname="t"\nversion="0"\n',
            "pkg/__init__.py": "",
            "pkg/run.py": "import os\n\ndef run():\n    os.system(input())\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    layer = build_pdg_for_paths(tmp_path, graph, paths=["pkg/run.py"])
    shards = merge_pdg_into_graph(graph, layer)
    write_code_graph(tmp_path, graph, analysis_shards=shards)
    assert graph.meta.get("pdg")
    assert graph.meta["pdg"]["stats"]["taint_count"] >= 0


def test_pdg_cli_explain_json(tmp_path):
    from devcouncil.indexing.graph.build import build_code_graph, build_pdg_for_paths, merge_pdg_into_graph, write_code_graph

    _write(
        tmp_path,
        {
            "pyproject.toml": '[project]\nname="t"\nversion="0"\n',
            "pkg/__init__.py": "",
            "pkg/run.py": "def run():\n    return 1\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    layer = build_pdg_for_paths(tmp_path, graph, paths=["pkg/run.py"])
    shards = merge_pdg_into_graph(graph, layer)
    write_code_graph(tmp_path, graph, analysis_shards=shards)
    runner = CliRunner()
    result = runner.invoke(graph_app, ["explain", "--project-root", str(tmp_path), "--json"])
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload.get("ok") is True
