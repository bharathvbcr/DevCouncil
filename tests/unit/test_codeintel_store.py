from __future__ import annotations

import sqlite3
from pathlib import Path

from devcouncil.codeintel.service import canonical_project_root, get_codeintel_service
from devcouncil.codeintel.store import CodeIntelStore
from devcouncil.indexing.graph.build import graph_path, load_code_graph, write_code_graph
from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind
from devcouncil.utils.json_persist import write_model_json


def _graph(path: str = "src/app.py", name: str = "main") -> CodeGraph:
    return CodeGraph(
        nodes=[
            GraphNode(id=path, kind=NodeKind.FILE, path=path, name=Path(path).name, language="python"),
            GraphNode(
                id=f"{path}::{name}",
                kind=NodeKind.FUNCTION,
                path=path,
                name=name,
                line=1,
                end_line=2,
                language="python",
            ),
        ],
        edges=[GraphEdge(source=path, target=f"{path}::{name}", kind="contains")],
        entry_roots=[path],
        generated_head="abc",
        indexed_hash="files",
        content_fingerprint="content",
        meta={"fixture": True},
    )


def test_store_round_trip_uses_wal_and_committed_generation(tmp_path: Path) -> None:
    source = tmp_path / "src" / "app.py"
    source.parent.mkdir()
    source.write_text("def main():\n    return 1\n", encoding="utf-8")
    store = CodeIntelStore(tmp_path)

    generation = store.save_graph(_graph())
    loaded = store.load_graph()

    assert generation == 1
    assert loaded is not None
    assert loaded.model_dump(exclude={"meta"}) == _graph().model_dump(exclude={"meta"})
    assert loaded.meta["fixture"] is True
    assert loaded.meta["codeintel_generation"] == generation
    assert store.content_for_path("src/app.py") == source.read_bytes()
    assert store.status().node_count == 2
    with sqlite3.connect(store.path) as conn:
        assert conn.execute("PRAGMA journal_mode").fetchone()[0] == "wal"


def test_new_generation_is_atomic_and_prunes_old_rows(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    first = store.save_graph(_graph(name="first"))
    second = store.save_graph(_graph(name="second"))
    third = store.save_graph(_graph(name="third"))

    assert (first, second, third) == (1, 2, 3)
    assert store.current_generation() == third
    assert store.load_graph().nodes[-1].name == "third"  # type: ignore[union-attr]
    assert store.load_graph(first) is None
    assert store.load_graph(second).nodes[-1].name == "second"  # type: ignore[union-attr]


def test_store_fts_and_extraction_cache(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph(name="request_handler"))

    hits = store.search("request_handler")
    assert hits[0]["name"] == "request_handler"

    store.put_extraction(
        content_hash="hash",
        language="python",
        grammar_version="1",
        config_hash="cfg",
        payload=b"payload",
    )
    assert store.get_extraction(
        content_hash="hash",
        language="python",
        grammar_version="1",
        config_hash="cfg",
    ) == b"payload"


def test_service_canonicalizes_nested_project_paths(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    nested = tmp_path / "src" / "pkg"
    nested.mkdir(parents=True)

    assert canonical_project_root(nested) == tmp_path.resolve()
    assert get_codeintel_service(nested) is get_codeintel_service(tmp_path)


def test_store_records_unresolved_dynamic_references_and_rename_aliases(tmp_path: Path) -> None:
    old = tmp_path / "old.py"
    old.write_text("value = eval(name)\n", encoding="utf-8")
    store = CodeIntelStore(tmp_path)
    first = CodeGraph(nodes=[
        GraphNode(id="old.py", kind=NodeKind.FILE, path="old.py", name="old.py", language="python"),
        GraphNode(
            id="old.py::dynamic:eval:1",
            kind=NodeKind.DYNAMIC,
            path="old.py",
            name="eval",
            line=1,
            end_line=1,
            language="python",
            extras={"resolved": False, "sink": "eval"},
        ),
    ], edges=[
        GraphEdge(source="old.py", target="old.py::dynamic:eval:1", kind="dynamic_reference")
    ])
    store.save_graph(first)

    new = tmp_path / "new.py"
    old.rename(new)
    second = first.model_copy(deep=True)
    for node in second.nodes:
        node.id = node.id.replace("old.py", "new.py")
        node.path = "new.py"
        if node.kind == NodeKind.FILE:
            node.name = "new.py"
    second.edges[0].source = "new.py"
    second.edges[0].target = "new.py::dynamic:eval:1"
    store.save_graph(second)

    assert store.unresolved_references() == [{
        "generation_id": 2,
        "source_id": "new.py",
        "name": "eval",
        "kind": "eval",
        "path": "new.py",
        "line": 1,
        "evidence": {"extras": {"resolved": False, "sink": "eval"}, "node_id": "new.py::dynamic:eval:1"},
    }]
    assert store.diagnostics()[0]["message"] == "Unresolved dynamic reference: eval"
    aliases = store.aliases()
    assert {row["old_id"]: row["new_id"] for row in aliases} == {
        "old.py": "new.py",
        "old.py::dynamic:eval:1": "new.py::dynamic:eval:1",
    }


def _file_node(path: str) -> GraphNode:
    return GraphNode(id=path, kind=NodeKind.FILE, path=path, name=Path(path).name, language="python")


def test_identical_content_surviving_files_do_not_alias(tmp_path: Path) -> None:
    """Two identical files present in both generations are not renames."""
    for rel in ("pkg_a/__init__.py", "pkg_b/__init__.py"):
        target = tmp_path / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("", encoding="utf-8")
    store = CodeIntelStore(tmp_path)
    graph = CodeGraph(nodes=[_file_node("pkg_a/__init__.py"), _file_node("pkg_b/__init__.py")])
    store.save_graph(graph)
    store.save_graph(graph.model_copy(deep=True))

    assert store.aliases() == []


def test_has_indexed_path_and_runtime_observation_gates(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    assert store.has_indexed_path("src/app.py") is False
    assert store.has_runtime_observations() is False

    source = tmp_path / "src" / "app.py"
    source.parent.mkdir()
    source.write_text("def main():\n    return 1\n", encoding="utf-8")
    store.save_graph(_graph())
    assert store.has_indexed_path("src/app.py") is True
    assert store.has_indexed_path("src\\app.py") is True
    assert store.has_indexed_path("src/other.py") is False

    assert store.has_runtime_observations() is False
    session = store.start_runtime_session(
        provider="pytest", source_fingerprint="fp", build_fingerprint="bp"
    )
    store.add_runtime_observations(session, [{"source": "a", "target": "b"}])
    assert store.has_runtime_observations() is True


def test_ambiguous_same_content_rename_is_not_aliased(tmp_path: Path) -> None:
    """One removed path matching two added identical files is not a provable rename."""
    old = tmp_path / "old.py"
    old.write_text("x = 1\n", encoding="utf-8")
    store = CodeIntelStore(tmp_path)
    store.save_graph(CodeGraph(nodes=[_file_node("old.py")]))

    old.unlink()
    for rel in ("first.py", "second.py"):
        (tmp_path / rel).write_text("x = 1\n", encoding="utf-8")
    store.save_graph(CodeGraph(nodes=[_file_node("first.py"), _file_node("second.py")]))

    assert store.aliases() == []


def test_store_disambiguates_duplicate_legacy_symbol_ids(tmp_path: Path) -> None:
    source = tmp_path / "app.py"
    source.write_text("def run(): pass\ndef run(): pass\n", encoding="utf-8")
    graph = CodeGraph(
        nodes=[
            GraphNode(id="app.py", kind=NodeKind.FILE, path="app.py", name="app.py"),
            GraphNode(id="app.py::run", kind=NodeKind.FUNCTION, path="app.py", name="run", line=1),
            GraphNode(id="app.py::run", kind=NodeKind.FUNCTION, path="app.py", name="run", line=2),
        ],
        edges=[
            GraphEdge(source="app.py", target="app.py::run", kind="contains", reason="ast definition"),
            GraphEdge(source="app.py", target="app.py::run", kind="contains", reason="ast definition"),
        ],
    )
    store = CodeIntelStore(tmp_path)

    store.save_graph(graph)
    loaded = store.load_graph()

    assert loaded is not None
    assert [node.id for node in loaded.nodes] == [
        "app.py",
        "app.py::run",
        "app.py::run#L2:function",
    ]
    assert loaded.meta["duplicate_symbol_aliases"] == [
        {
            "new_id": "app.py::run#L2:function",
            "old_id": "app.py::run",
            "reason": "duplicate legacy symbol identity",
        }
    ]
    assert any(
        edge.kind == "aliases"
        and edge.source == "app.py::run#L2:function"
        and edge.target == "app.py::run"
        for edge in loaded.edges
    )


def test_compatibility_export_is_not_reimported_but_external_replacement_is(tmp_path: Path) -> None:
    source = tmp_path / "src" / "app.py"
    source.parent.mkdir()
    source.write_text("def main(): return 1\n", encoding="utf-8")
    initial = _graph()

    write_code_graph(tmp_path, initial)
    service = get_codeintel_service(tmp_path)
    assert service.store.current_generation() == 1
    assert load_code_graph(tmp_path) is not None
    assert load_code_graph(tmp_path) is not None
    assert service.store.current_generation() == 1

    # External JSON rewrite must not clobber the canonical store.
    replacement = _graph(name="replacement")
    write_model_json(graph_path(tmp_path), replacement)
    loaded = load_code_graph(tmp_path)

    assert loaded is not None
    assert loaded.nodes[-1].name == "main"
    assert service.store.current_generation() == 1


def test_compatibility_json_imports_only_when_store_missing(tmp_path: Path) -> None:
    source = tmp_path / "src" / "app.py"
    source.parent.mkdir()
    source.write_text("def main(): return 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil" / "graph").mkdir(parents=True)
    write_model_json(graph_path(tmp_path), _graph(name="from_json"))

    loaded = load_code_graph(tmp_path)
    service = get_codeintel_service(tmp_path)
    assert loaded is not None
    assert loaded.nodes[-1].name == "from_json"
    assert service.store.current_generation() == 1


def test_incremental_generation_reuses_content_addressed_payloads(tmp_path: Path) -> None:
    source = tmp_path / "src" / "app.py"
    source.parent.mkdir()
    source.write_text("def main():\n    return 1\n", encoding="utf-8")
    store = CodeIntelStore(tmp_path)
    graph = _graph()
    store.save_graph(graph)

    source.write_text("def main():\n    return 2\n", encoding="utf-8")
    changed = graph.model_copy(deep=True)
    changed.nodes[-1].end_line = 3
    store.save_graph(changed, changed_paths={"src/app.py"})

    assert store.last_write_stats == {
        "node_payloads_written": 1,
        "edge_payloads_written": 0,
        "dead_payloads_written": 0,
        "node_memberships": 2,
        "edge_memberships": 1,
    }
    with sqlite3.connect(store.path) as conn:
        assert conn.execute("SELECT COUNT(*) FROM node_payloads").fetchone()[0] == 3
        assert conn.execute("SELECT COUNT(*) FROM edge_payloads").fetchone()[0] == 1
        assert conn.execute("SELECT COUNT(*) FROM file_contents").fetchone()[0] == 2
        assert conn.execute("SELECT COUNT(*) FROM diagnostics").fetchone()[0] == 0


def test_pruning_reclaims_unreferenced_payload_rows(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    for name in ("first", "second", "third"):
        store.save_graph(_graph(name=name))

    with sqlite3.connect(store.path) as conn:
        retained = conn.execute(
            "SELECT COUNT(*) FROM generations WHERE state='committed'"
        ).fetchone()[0]
        node_payloads = conn.execute("SELECT COUNT(*) FROM node_payloads").fetchone()[0]
        referenced = conn.execute(
            "SELECT COUNT(DISTINCT payload_hash) FROM generation_nodes"
        ).fetchone()[0]
    assert retained == 2
    assert node_payloads == referenced


def test_v1_store_migrates_without_rebuilding_graph(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    store.initialize()
    with sqlite3.connect(store.path) as conn:
        for table in (
            "generation_analysis", "analysis_payloads", "generation_dead",
            "dead_payloads", "generation_edges", "edge_payloads",
            "generation_nodes", "node_payloads", "generation_files",
            "file_contents",
        ):
            conn.execute(f"DROP TABLE {table}")
        conn.execute("PRAGMA user_version=1")
        conn.execute(
            """INSERT INTO generations(
                id, state, created_at, analyzer_version, schema_version,
                graph_meta, node_count, edge_count
            ) VALUES(1, 'committed', 1.0, 'codeintel-1', 2, '{}', 1, 0)"""
        )
        conn.execute(
            "INSERT INTO metadata(key, value) VALUES('current_generation', '1')"
        )
        conn.execute(
            """INSERT INTO nodes(
                generation_id, id, kind, path, name, extras
            ) VALUES(1, 'legacy.py', 'file', 'legacy.py', 'legacy.py', '{}')"""
        )
        conn.commit()

    store.initialize()
    loaded = store.load_graph()

    assert loaded is not None
    assert [node.id for node in loaded.nodes] == ["legacy.py"]
    assert store.status().schema_version == 2


def test_failed_compact_generation_keeps_previous_generation(
    tmp_path: Path,
    monkeypatch,
) -> None:
    store = CodeIntelStore(tmp_path)
    first = store.save_graph(_graph(name="first"))

    def fail(_node):
        raise RuntimeError("injected write failure")

    monkeypatch.setattr(store, "_node_payload", fail)
    try:
        store.save_graph(_graph(name="second"))
    except RuntimeError as exc:
        assert str(exc) == "injected write failure"
    else:
        raise AssertionError("save unexpectedly succeeded")

    assert store.current_generation() == first
    assert store.load_graph().nodes[-1].name == "first"  # type: ignore[union-attr]


def test_empty_changed_paths_must_not_drop_memberships(tmp_path: Path) -> None:
    """changed_paths=set() must full-persist, not commit an empty incremental gen."""
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph(name="first"))
    fuller = _graph(name="second")
    fuller.nodes.append(
        GraphNode(
            id="src/other.py",
            kind=NodeKind.FILE,
            path="src/other.py",
            name="other.py",
            language="python",
        )
    )
    store.save_graph(fuller, changed_paths=set())
    loaded = store.load_graph()
    assert loaded is not None
    assert len(loaded.nodes) == 3
    assert {node.id for node in loaded.nodes} >= {"src/app.py", "src/app.py::second", "src/other.py"}
    assert store.last_write_stats["node_memberships"] == 3
