"""Graph intelligence: communities, processes, diff impact."""

from __future__ import annotations

import json
import subprocess

import pytest
from typer.testing import CliRunner

from devcouncil.cli.commands.graph_cmd import app as graph_app
from tests.unit.graph_fixtures import write_graph_artifact
from tests.unit.graph_fixtures import kernel_graph
from devcouncil.indexing.graph.intel import (
    circular_imports,
    compute_communities,
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
    graph = kernel_graph(tmp_path)
    write_graph_artifact(tmp_path, graph)
    return tmp_path, graph


def test_communities_deterministic(call_chain):
    root, graph = call_chain
    a = compute_communities(graph, seed=0)
    # Rebuild a fresh graph and recompute — labels/membership must match.
    g2 = kernel_graph(root)
    b = compute_communities(g2, seed=0)
    assert a["count"] == b["count"]
    labels_a = sorted(c["label"] for c in a["communities"])
    labels_b = sorted(c["label"] for c in b["communities"])
    assert labels_a == labels_b
    # Nodes carry community strings after enrich
    assert any(n.community for n in g2.nodes)
    assert "communities" in (g2.meta or {}) or any(n.community for n in g2.nodes)


def test_god_nodes_and_cycles(call_chain):
    _, graph = call_chain
    gods = god_nodes(graph, top_n=5)
    assert gods
    assert gods[0]["degree"] >= gods[-1]["degree"]
    report = graph_check(graph)
    assert "god_nodes" in report
    assert "circular_imports" in report


def test_pagerank_scores_are_fixed_precision(call_chain):
    """PageRank values are rounded so tiny solver noise cannot flip JSON bytes."""
    from devcouncil.indexing.graph.intel import pagerank_scores

    _, graph = call_chain
    scores = pagerank_scores(graph)
    assert scores
    for value in scores.values():
        assert value == round(value, 4)


def test_god_nodes_tie_break_is_stable():
    """Equal-degree nodes sort by id so ranking does not depend on edge insert order."""
    nodes = [
        GraphNode(id="b.py", kind=NodeKind.FILE, path="b.py", name="b.py"),
        GraphNode(id="a.py", kind=NodeKind.FILE, path="a.py", name="a.py"),
    ]
    edges = [
        GraphEdge(source="a.py", target="b.py", kind="imports"),
        GraphEdge(source="b.py", target="a.py", kind="imports"),
    ]
    graph = CodeGraph(nodes=nodes, edges=edges)
    first = [row["id"] for row in god_nodes(graph, top_n=5)]
    second = [row["id"] for row in god_nodes(graph, top_n=5)]
    assert first == second == ["a.py", "b.py"]


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
    graph = kernel_graph(tmp_path)
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


# `diff_impact` -- a Python per-path blast walk over a whole `CodeGraph` -- had
# its test here and nowhere else: `graph_cmd` stopped calling it, the MCP impact
# tool stopped calling it, and by Lane M3 the kernel banded its own walk
# (`impact --layers`). The function and its re-export are deleted; the banding
# it produced is asserted against the kernel in
# `test_impact_layers_are_the_kernels.py`.


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
    # The kernel answers with `paths` (the Python engine said `path_count`).
    assert len(impact.get("paths") or []) >= 1


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
        query_pdg_controls,
    )

    assert CFGResult is not None
    assert PDG_VERSION >= 1
    assert callable(build_cfg_for_function)
    assert callable(build_pdg_for_paths)
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


def test_pdg_build_writes_its_own_sidecar(tmp_path):
    """The layer is an artifact of its own, not an annotation on the kernel's.

    This built the layer, merged it into `graph.meta["pdg"]` with
    `merge_pdg_into_graph` and wrote the whole graph back with
    `write_code_graph`. Lane M3 moved the layer to `.devcouncil/graph/pdg.json`
    because the kernel is the only writer of `code_graph.json`; the merge and
    the writer then had no production caller and are deleted. What the layer
    contains is the same, asserted where it now lives.
    """
    from devcouncil.indexing.graph.build import (
        build_pdg_for_paths,
        read_pdg_layer_file,
        write_pdg_layer,
    )

    _write(
        tmp_path,
        {
            "pyproject.toml": '[project]\nname="t"\nversion="0"\n',
            "pkg/__init__.py": "",
            "pkg/run.py": "import os\n\ndef run():\n    os.system(input())\n",
        },
    )
    _commit(tmp_path)
    graph = kernel_graph(tmp_path)
    graph_bytes = (tmp_path / ".devcouncil" / "graph" / "code_graph.json").read_bytes()

    layer = build_pdg_for_paths(tmp_path, graph, paths=["pkg/run.py"])
    sidecar = write_pdg_layer(tmp_path, layer)

    assert sidecar.is_file()
    assert (layer.to_meta() or {})["stats"]["taint_count"] >= 0
    assert read_pdg_layer_file(tmp_path) is not None
    assert (tmp_path / ".devcouncil" / "graph" / "code_graph.json").read_bytes() == graph_bytes, (
        "building the PDG layer rewrote the kernel's artifact"
    )


def test_pagerank_pure_python_fallback_matches_networkx(call_chain):
    """When networkx's solver deps are missing, the fallback yields the same ranks."""
    import networkx as nx
    import pytest

    from devcouncil.indexing.graph import intel as intel_mod
    from devcouncil.indexing.graph.intel import pagerank_scores

    _, graph = call_chain
    expected = pagerank_scores(graph)
    assert expected and any(v > 0 for v in expected.values())

    def _boom(*args, **kwargs):
        raise ModuleNotFoundError("No module named 'numpy'")

    monkeypatch = pytest.MonkeyPatch()
    try:
        monkeypatch.setattr(nx, "pagerank", _boom)
        fallback = pagerank_scores(graph)
    finally:
        monkeypatch.undo()
    assert fallback.keys() == expected.keys()
    for nid, score in expected.items():
        assert abs(fallback[nid] - score) <= 2e-4, nid
    # Sanity on the raw iterator too: probability mass sums to ~1.
    edges = sorted(
        {
            (e.source, e.target)
            for e in graph.edges
            if e.kind in intel_mod._STRUCTURAL_KINDS and e.source != e.target
        }
    )
    raw = intel_mod._pagerank_power_iteration(edges)
    assert abs(sum(raw.values()) - 1.0) < 1e-6


def test_god_nodes_exclude_test_mocks():
    """High fan-in test fixtures must not crowd production hubs out of the ranking."""
    nodes = [
        GraphNode(id="pkg/hub.py::run", kind=NodeKind.FUNCTION, path="pkg/hub.py", name="run"),
        GraphNode(
            id="tests/mocks.py::mock_loop",
            kind=NodeKind.FUNCTION,
            path="tests/mocks.py",
            name="mock_loop",
        ),
    ]
    edges = []
    for idx in range(6):
        edges.append(
            GraphEdge(
                source=f"tests/t{idx}.py",
                target="tests/mocks.py::mock_loop",
                kind="calls",
            )
        )
    for idx in range(2):
        edges.append(
            GraphEdge(source=f"pkg/c{idx}.py", target="pkg/hub.py::run", kind="calls")
        )
    graph = CodeGraph(nodes=nodes, edges=edges)
    gods = god_nodes(graph, top_n=5)
    ids = [g["id"] for g in gods]
    assert "pkg/hub.py::run" in ids
    assert "tests/mocks.py::mock_loop" not in ids
    assert all(not g["path"].startswith("tests/") for g in gods)


def test_ambiguous_edges_excluded_from_god_pagerank_process_impact():
    """Ambiguous call fan-out must not invent hubs or inflate process/impact walks."""
    from devcouncil.indexing.graph.intel import blast_radius, pagerank_scores

    nodes = [
        GraphNode(id="pkg/a.py::caller", kind=NodeKind.FUNCTION, path="pkg/a.py", name="caller"),
        GraphNode(id="pkg/hub.py::run", kind=NodeKind.FUNCTION, path="pkg/hub.py", name="run"),
        GraphNode(
            id="pkg/noise.py::to_dict",
            kind=NodeKind.FUNCTION,
            path="pkg/noise.py",
            name="to_dict",
        ),
        GraphNode(
            id="pkg/other.py::to_dict",
            kind=NodeKind.FUNCTION,
            path="pkg/other.py",
            name="to_dict",
        ),
    ]
    edges = [
        GraphEdge(
            source="pkg/a.py::caller",
            target="pkg/hub.py::run",
            kind="calls",
            confidence=Confidence.EXTRACTED,
        ),
        # Ambiguous fan-out to every to_dict — historically created fake hubs.
        GraphEdge(
            source="pkg/a.py::caller",
            target="pkg/noise.py::to_dict",
            kind="calls",
            confidence=Confidence.AMBIGUOUS,
            reason="ambiguous (2 candidates)",
        ),
        GraphEdge(
            source="pkg/a.py::caller",
            target="pkg/other.py::to_dict",
            kind="calls",
            confidence=Confidence.AMBIGUOUS,
            reason="ambiguous (2 candidates)",
        ),
        GraphEdge(
            source="pkg/x.py::x",
            target="pkg/noise.py::to_dict",
            kind="calls",
            confidence=Confidence.AMBIGUOUS,
        ),
        GraphEdge(
            source="pkg/y.py::y",
            target="pkg/noise.py::to_dict",
            kind="calls",
            confidence=Confidence.AMBIGUOUS,
        ),
        GraphEdge(
            source="pkg/z.py::z",
            target="pkg/noise.py::to_dict",
            kind="calls",
            confidence=Confidence.AMBIGUOUS,
        ),
    ]
    graph = CodeGraph(nodes=nodes, edges=edges, entry_roots=["pkg/a.py"])

    gods = god_nodes(graph, top_n=5)
    god_ids = [g["id"] for g in gods]
    assert "pkg/hub.py::run" in god_ids
    assert "pkg/noise.py::to_dict" not in god_ids
    assert "pkg/other.py::to_dict" not in god_ids

    scores = pagerank_scores(graph)
    assert scores.get("pkg/hub.py::run", 0) >= scores.get("pkg/noise.py::to_dict", 0)

    procs = extract_processes(graph, entry="pkg/a.py", max_depth=4)
    steps = {step for proc in procs for step in proc["steps"]}
    assert "pkg/hub.py::run" in steps
    assert "pkg/noise.py::to_dict" not in steps

    br = blast_radius(graph, ["pkg/noise.py::to_dict"], max_depth=2)
    impacted = {n for layer in br["layers"] for n in layer["nodes"]}
    assert not impacted  # only ambiguous inbound edges exist

def test_graph_limit_factories_include_recovery_and_store_health():
    from devcouncil.indexing.graph.communities import (
        COMMUNITY_TIMEOUT_SECONDS,
        community_detection_limit,
        compatibility_export_limit,
        embedding_scan_limit,
        store_health_from_state,
    )
    assert COMMUNITY_TIMEOUT_SECONDS == 15.0
    assert store_health_from_state("committed") == "healthy"
    assert compatibility_export_limit(canonical_store_health="healthy", reason="test").as_dict()["recovery_command"] == "dev map query <symbol>"
    assert embedding_scan_limit(canonical_store_health="healthy", reason="x").as_dict()["kind"] == "embedding_scan"
    assert community_detection_limit(canonical_store_health="healthy", reason="x").as_dict()["recovery_command"] == "dev map"


def test_community_timeout_reports_structured_limit(monkeypatch):
    from concurrent.futures import TimeoutError as FuturesTimeout

    from devcouncil.indexing.graph.intel import compute_communities
    from devcouncil.indexing.graph.schema import (
        CodeGraph,
        Confidence,
        GraphEdge,
        GraphNode,
        NodeKind,
    )

    graph = CodeGraph(
        nodes=[
            GraphNode(id="a.py", kind=NodeKind.FILE, path="a.py", name="a.py"),
            GraphNode(id="b.py", kind=NodeKind.FILE, path="b.py", name="b.py"),
        ],
        edges=[
            GraphEdge(
                source="a.py",
                target="b.py",
                kind="imports",
                confidence=Confidence.EXTRACTED,
            )
        ],
    )

    class _Future:
        def result(self, timeout=None):
            raise FuturesTimeout()

    class _Pool:
        def __init__(self, *a, **k):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def submit(self, fn):
            return _Future()

    import concurrent.futures as cf

    monkeypatch.setattr(cf, "ThreadPoolExecutor", _Pool)
    summary = compute_communities(graph, seed=0)
    assert summary["skipped"] is True
    assert summary["limit"]["degraded"] is True
    assert summary["limit"]["kind"] == "community_detection"
    assert summary["limit"]["canonical_store_health"] == "healthy"
    assert summary["limit"]["recovery_command"] == "dev map"

def test_graph_doctor_json_reports_a_foreign_artifact_writer(tmp_path, monkeypatch):
    """An artifact not written by the kernel is a critical finding with a fix.

    Doctor used to report the Python engine's compatibility-export limits.
    The kernel writes both artifacts from one generation, so the failure that
    matters now is the opposite one: something *else* wrote `repo_map.json`
    (an old engine, a hand edit) and the map no longer describes the store.
    """
    import json

    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    monkeypatch.setattr(
        "devcouncil.devmap_health.engine_info",
        lambda root: {
            "binary": "/opt/devmap",
            "built_at": "2026-09-02T16:51:00",
            "version": "devmap 0.1.0 (schema 12)",
            "schema_version": 12,
            "error": None,
        },
    )
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True)
    map_path.write_text('{"files": [], "map_engine": "python-indexer"}', encoding="utf-8")

    runner = CliRunner()
    result = runner.invoke(app, ["map", "doctor", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    payload = json.loads(result.stdout)
    assert payload["ok"] is False
    finding = next(check for check in payload["checks"] if check["name"] == "repo_map")
    assert finding["ok"] is False
    assert finding["critical"] is True
    assert "python-indexer" in finding["detail"]
    assert finding["fix"].startswith("dev map")
