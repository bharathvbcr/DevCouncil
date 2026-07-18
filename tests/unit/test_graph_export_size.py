"""Graph JSON export size: candidate dedupe, compact write, slim meta."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from devcouncil.indexing.graph.build import (
    CompatibilityGraphTooLarge,
    _write_graph_json_bounded,
    _slim_graph_export,
    build_code_graph,
    extract_all,
    write_code_graph,
)
from devcouncil.indexing.graph.resolve import (
    AMBIGUOUS_CANDIDATES_CAP,
    _stable_candidate_ids,
    build_file_and_symbol_nodes,
    named_import_edges,
    resolve_calls,
    resolve_import_edges,
)
from devcouncil.indexing.graph.schema import CodeGraph, Confidence, GraphNode, NodeKind


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


def test_ambiguous_fanout_attaches_candidates_once(tmp_path: Path):
    """Fan-out keeps all edges; ``candidates`` extras appear on at most one edge."""
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    for name in ("a", "b", "c"):
        (pkg / f"{name}.py").write_text(f"def shared():\n    return {name!r}\n", encoding="utf-8")
    (pkg / "caller.py").write_text("def caller():\n    return shared()\n", encoding="utf-8")
    files = [f"pkg/{n}.py" for n in ("__init__", "a", "b", "c", "caller")]
    extractions = extract_all(tmp_path, files)
    _nodes, index = build_file_and_symbol_nodes(extractions)
    edges = resolve_calls(extractions, index, [])
    ambig = [e for e in edges if e.kind == "calls" and e.confidence == Confidence.AMBIGUOUS]
    assert len(ambig) >= 2
    with_cands = [e for e in ambig if e.extras.get("candidates")]
    assert len(with_cands) == 1
    assert len(with_cands[0].extras["candidates"]) >= 2
    assert len(with_cands[0].extras["candidates"]) <= AMBIGUOUS_CANDIDATES_CAP


def test_ambiguous_candidates_capped(tmp_path: Path):
    """More than CAP same-name defs → extras list truncated with truncated count."""
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    n = AMBIGUOUS_CANDIDATES_CAP + 4
    for i in range(n):
        (pkg / f"m{i}.py").write_text("def shared():\n    return 1\n", encoding="utf-8")
    (pkg / "caller.py").write_text("def caller():\n    return shared()\n", encoding="utf-8")
    files = ["pkg/__init__.py"] + [f"pkg/m{i}.py" for i in range(n)] + ["pkg/caller.py"]
    extractions = extract_all(tmp_path, files)
    _nodes, index = build_file_and_symbol_nodes(extractions)
    edges = resolve_calls(extractions, index, [])
    ambig = [e for e in edges if e.kind == "calls" and e.confidence == Confidence.AMBIGUOUS]
    with_cands = [e for e in ambig if e.extras.get("candidates")]
    assert len(with_cands) == 1
    assert len(with_cands[0].extras["candidates"]) == AMBIGUOUS_CANDIDATES_CAP
    assert with_cands[0].extras.get("candidates_truncated", 0) >= 4
    # Fan-out still reaches every candidate for liveness.
    targets = {e.target for e in ambig}
    assert len(targets) >= n


def test_ambiguous_fanout_candidates_are_sorted(tmp_path: Path):
    """Ambiguous fan-out targets and extras.candidates follow sorted id order."""
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    # Reverse creation order vs lexicographic id order.
    for name in ("z", "m", "a"):
        (pkg / f"{name}.py").write_text(f"def shared():\n    return {name!r}\n", encoding="utf-8")
    (pkg / "caller.py").write_text("def caller():\n    return shared()\n", encoding="utf-8")
    files = [f"pkg/{n}.py" for n in ("__init__", "z", "m", "a", "caller")]
    extractions = extract_all(tmp_path, files)
    _nodes, index = build_file_and_symbol_nodes(extractions)
    edges = resolve_calls(extractions, index, [])
    ambig = [e for e in edges if e.kind == "calls" and e.confidence == Confidence.AMBIGUOUS]
    targets = [e.target for e in ambig]
    assert targets == sorted(targets)
    with_cands = [e for e in ambig if e.extras.get("candidates")]
    assert len(with_cands) == 1
    assert with_cands[0].extras["candidates"] == _stable_candidate_ids(
        with_cands[0].extras["candidates"]
    )


def test_named_import_skips_submodule_and_is_deterministic(tmp_path: Path):
    """``from pkg import show`` must not bind a sibling ``show()`` hash-order flip."""
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "show.py").write_text("def run():\n    return 1\n", encoding="utf-8")
    (pkg / "cost.py").write_text("def show():\n    return 2\n", encoding="utf-8")
    (pkg / "design.py").write_text("def show():\n    return 3\n", encoding="utf-8")
    (pkg / "main.py").write_text(
        "from pkg import show, cost, design\n\ndef main():\n    return show\n",
        encoding="utf-8",
    )
    files = [f"pkg/{n}.py" for n in ("__init__", "show", "cost", "design", "main")]
    extractions = extract_all(tmp_path, files)
    _nodes, index = build_file_and_symbol_nodes(extractions)
    file_edges = resolve_import_edges(extractions, files, root=tmp_path)
    edges = named_import_edges(extractions, index, file_edges)
    named = [e for e in edges if e.reason == "named import" and e.source == "pkg/main.py"]
    targets = {e.target for e in named}
    assert "pkg/cost.py::show" not in targets
    assert "pkg/design.py::show" not in targets
    # Stable across repeated resolution.
    again = named_import_edges(extractions, index, file_edges)
    assert [(e.source, e.target) for e in edges] == [(e.source, e.target) for e in again]


def test_slim_graph_export_drops_bulky_meta_and_pagerank():
    graph = CodeGraph(
        nodes=[GraphNode(id="a.py", kind=NodeKind.FILE, name="a.py", path="a.py", community="c1")],
        edges=[],
        meta={
            "node_communities": {"a.py": "c1"},
            "legacy_dead_symbol_candidates": ["a.py::orphan"],
            "file_edge_count": 0,
            "god_nodes": [{"id": "a.py", "degree": 3, "pagerank": 0.015431}],
            "incremental": True,
            "affected_paths": ["a.py"],
        },
    )
    slim = _slim_graph_export(graph)
    assert "node_communities" not in slim.meta
    assert "legacy_dead_symbol_candidates" not in slim.meta
    assert slim.meta.get("file_edge_count") == 0
    assert slim.meta.get("god_nodes") == [{"id": "a.py", "degree": 3}]
    assert slim.meta.get("incremental") is True
    assert slim.meta.get("compatibility_export_tier") == "slim"
    # In-memory original unchanged.
    assert "node_communities" in graph.meta
    assert graph.meta["god_nodes"][0]["pagerank"] == 0.015431
    assert graph.nodes[0].community == "c1"


def test_tiered_export_writes_stub_when_full_exceeds_limit(tmp_path: Path, monkeypatch):
    """Oversized graphs still leave a stub JSON and surface CompatibilityGraphTooLarge."""
    import devcouncil.indexing.graph.build as build

    _write(
        tmp_path,
        {
            "pkg/__init__.py": "",
            "pkg/main.py": "def main():\n    return 1\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    # Force every non-stub tier to fail the byte cap.
    monkeypatch.setattr(build, "_graph_json_max_bytes", lambda _root: 256)
    path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    try:
        write_code_graph(tmp_path, graph)
    except CompatibilityGraphTooLarge as exc:
        assert "stub" in str(exc).lower()
    else:
        raise AssertionError("expected CompatibilityGraphTooLarge after stub write")
    assert path.is_file()
    data = json.loads(path.read_text(encoding="utf-8"))
    assert (data.get("meta") or {}).get("compatibility_export_tier") == "stub"
    assert data.get("nodes") == []
    assert (data.get("meta") or {}).get("sqlite_canonical") is True


def test_compact_graph_export_strips_extras_and_unreachable():
    from devcouncil.indexing.graph.build import _compact_graph_export
    from devcouncil.indexing.graph.schema import GraphEdge

    graph = CodeGraph(
        nodes=[
            GraphNode(
                id="a.py",
                kind=NodeKind.FILE,
                name="a.py",
                path="a.py",
                extras={"heavy": "x" * 100},
            )
        ],
        edges=[
            GraphEdge(
                source="a.py",
                target="b.py",
                kind="imports",
                reason="import",
                extras={"candidates": ["c1", "c2"]},
            )
        ],
        unreachable_files=["orphan.py"],
        unwired_candidates=[f"u{i}.py" for i in range(250)],
        meta={"legacy_dead_symbol_candidates": ["x"]},
    )
    compact = _compact_graph_export(graph)
    assert compact.nodes[0].extras == {}
    assert compact.edges[0].extras == {}
    assert compact.edges[0].reason == ""
    assert compact.unreachable_files == []
    assert len(compact.unwired_candidates) == 200
    assert compact.meta.get("compatibility_export_tier") == "compact"


def test_incremental_write_uses_slim_compact_export(tmp_path: Path):
    """Incremental sync must write the same slim+compact JSON as full map."""
    _write(
        tmp_path,
        {
            "pyproject.toml": (
                '[project]\nname="t"\nversion="0"\n'
                '[project.scripts]\ncli="pkg.main:main"\n'
            ),
            "pkg/__init__.py": "",
            "pkg/main.py": "def main():\n    return 1\n",
            "pkg/util.py": "def helper():\n    return 2\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=True)
    graph.meta["node_communities"] = {"x": "y"}
    graph.meta["legacy_dead_symbol_candidates"] = ["pkg/main.py::ghost"]
    graph.meta["god_nodes"] = [{"id": "pkg/main.py", "degree": 1, "pagerank": 0.123456}]
    graph.meta["incremental"] = True
    path = write_code_graph(tmp_path, graph)
    raw = path.read_text(encoding="utf-8")
    data = json.loads(raw)
    assert "node_communities" not in (data.get("meta") or {})
    assert "legacy_dead_symbol_candidates" not in (data.get("meta") or {})
    gods = (data.get("meta") or {}).get("god_nodes") or []
    assert gods and "pagerank" not in gods[0]
    assert data["meta"].get("incremental") is True
    assert '\n  "' not in raw or raw.count("\n") < 50


def test_write_code_graph_compact_and_slim(tmp_path: Path):
    _write(
        tmp_path,
        {
            "pyproject.toml": (
                '[project]\nname="t"\nversion="0"\n'
                '[project.scripts]\ncli="pkg.main:main"\n'
            ),
            "pkg/__init__.py": "",
            "pkg/main.py": "def main():\n    return 1\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=True)
    graph.meta["node_communities"] = {"x": "y"}
    graph.meta["legacy_dead_symbol_candidates"] = ["pkg/main.py::ghost"]
    path = write_code_graph(tmp_path, graph)
    raw = path.read_text(encoding="utf-8")
    # Compact: no pretty-print indent (lines are long / few newlines relative to size).
    data = json.loads(raw)
    assert "node_communities" not in (data.get("meta") or {})
    assert "legacy_dead_symbol_candidates" not in (data.get("meta") or {})
    # Pretty JSON would start objects on their own indented lines; compact packs keys.
    assert '\n  "' not in raw or raw.count("\n") < 50


def test_bounded_graph_export_preserves_previous_artifact(tmp_path: Path):
    path = tmp_path / "code_graph.json"
    path.write_text('{"previous":true}\n', encoding="utf-8")
    graph = CodeGraph(
        nodes=[GraphNode(id="a.py", kind=NodeKind.FILE, name="a.py", path="a.py")],
        edges=[],
    )

    try:
        _write_graph_json_bounded(path, graph, indent=None, max_bytes=16)
    except CompatibilityGraphTooLarge:
        pass
    else:
        raise AssertionError("expected bounded export failure")

    assert path.read_text(encoding="utf-8") == '{"previous":true}\n'
    assert not list(tmp_path.glob(".code_graph.json.*.tmp"))


def test_oversized_compatibility_import_is_rejected_before_read(tmp_path: Path, monkeypatch):
    import devcouncil.indexing.graph.build as build

    path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    path.parent.mkdir(parents=True)
    path.write_bytes(b"x" * 32)
    monkeypatch.setattr(build, "_graph_json_max_bytes", lambda _root: 16)
    monkeypatch.setattr(
        build,
        "read_json",
        lambda _path: (_ for _ in ()).throw(AssertionError("oversized JSON was read")),
    )

    assert build.load_code_graph(tmp_path) is None


def test_export_code_graph_json_self_heals_deleted_artifact(tmp_path: Path):
    """A deleted JSON export is restored from the canonical SQLite store."""
    from devcouncil.indexing.graph.build import export_code_graph_json, load_code_graph

    _write(
        tmp_path,
        {
            "pkg/__init__.py": "",
            "pkg/main.py": "def main():\n    return 1\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    path = write_code_graph(tmp_path, graph)
    assert path.is_file()
    path.unlink()

    healed = export_code_graph_json(tmp_path)
    assert healed is not None and healed.is_file()
    restored = load_code_graph(tmp_path)
    assert restored is not None
    assert {n.id for n in restored.nodes} >= {n.id for n in graph.nodes if "::" in n.id}


def test_export_code_graph_json_returns_none_without_store(tmp_path: Path):
    from devcouncil.indexing.graph.build import export_code_graph_json

    _write(tmp_path, {"pkg/__init__.py": ""})
    _commit(tmp_path)
    assert export_code_graph_json(tmp_path) is None
