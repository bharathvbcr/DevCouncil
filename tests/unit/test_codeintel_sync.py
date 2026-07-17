from __future__ import annotations

from pathlib import Path
import subprocess
import threading
import time

from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.codeintel.sync import IndexScope, SyncCoordinator
from devcouncil.codeintel.sync.incremental import sync_affected_paths
from devcouncil.codeintel.sync.lease import WriterLease
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind


def _persist_file(root: Path, rel: str) -> None:
    service = get_codeintel_service(root)
    service.persist(CodeGraph(nodes=[
        GraphNode(id=rel, kind=NodeKind.FILE, path=rel, name=Path(rel).name, language="python")
    ]))


def test_index_scope_uses_language_manifest_and_ignores_state(tmp_path: Path) -> None:
    (tmp_path / ".git").mkdir()
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "src" / "note.txt").write_text("no\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "x.py").write_text("ignored\n", encoding="utf-8")

    scope = IndexScope(tmp_path)
    assert scope.includes("src/a.py")
    assert not scope.includes("src/note.txt")
    assert not scope.includes(".devcouncil/x.py")


def test_index_scope_files_does_not_recheck_git_ignored_paths(tmp_path: Path, monkeypatch) -> None:
    scope = IndexScope(tmp_path)
    monkeypatch.setattr(
        subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 0, stdout=b"a.py\0note.txt\0"),
    )
    monkeypatch.setattr(
        scope,
        "_git_ignored",
        lambda _rel: (_ for _ in ()).throw(AssertionError("redundant check-ignore")),
    )

    assert scope.files() == ["a.py"]


def test_coordinator_start_does_not_block_on_initial_reconcile(tmp_path: Path, monkeypatch) -> None:
    coordinator = SyncCoordinator(get_codeintel_service(tmp_path), sync_callback=lambda _paths: None)
    entered = threading.Event()
    release = threading.Event()

    def slow_reconcile():
        entered.set()
        release.wait(timeout=2)
        return []

    monkeypatch.setattr(coordinator, "_start_observer", lambda: None)
    monkeypatch.setattr(coordinator, "reconcile", slow_reconcile)
    started_at = time.monotonic()
    coordinator.start()

    assert time.monotonic() - started_at < 0.2
    assert entered.wait(timeout=1)
    release.set()
    coordinator.stop()


def test_reconcile_detects_modify_create_and_delete(tmp_path: Path) -> None:
    source = tmp_path / "a.py"
    source.write_text("x = 1\n", encoding="utf-8")
    _persist_file(tmp_path, "a.py")
    source.write_text("x = 2\n", encoding="utf-8")
    created = tmp_path / "b.py"
    created.write_text("y = 1\n", encoding="utf-8")
    coordinator = SyncCoordinator(get_codeintel_service(tmp_path), sync_callback=lambda _paths: None)

    changed = coordinator.reconcile()
    assert changed == ["a.py", "b.py"]

    source.unlink()
    assert "a.py" in coordinator.reconcile()


def test_sync_batches_pending_and_updates_state(tmp_path: Path) -> None:
    source = tmp_path / "a.py"
    source.write_text("x = 1\n", encoding="utf-8")
    seen: list[list[str]] = []
    coordinator = SyncCoordinator(
        get_codeintel_service(tmp_path),
        debounce_seconds=0.1,
        sync_callback=lambda paths: seen.append(paths),
    )
    coordinator.mark_pending("a.py")

    assert coordinator.sync_now()
    assert seen == [["a.py"]]
    assert coordinator.status().pending == []


def test_watcher_polling_fallback_is_reported_separately(tmp_path: Path, monkeypatch) -> None:
    import importlib

    import watchdog.observers
    import watchdog.observers.polling

    class FailingObserver:
        emitters: list[object] = []

        def schedule(self, *_args, **_kwargs):
            raise OSError("native unavailable")

        def stop(self):
            return None

        def join(self, **_kwargs):
            return None

    class FakePollingObserver:
        emitters: list[object] = []

        def __init__(self, **_kwargs):
            return None

        def schedule(self, *_args, **_kwargs):
            return None

        def start(self):
            return None

        def stop(self):
            return None

        def join(self, **_kwargs):
            return None

    monkeypatch.setattr(watchdog.observers, "Observer", FailingObserver)
    try:
        kqueue = importlib.import_module("watchdog.observers.kqueue")
    except ImportError:
        pass
    else:
        monkeypatch.setattr(kqueue, "KqueueObserver", FailingObserver)
    monkeypatch.setattr(watchdog.observers.polling, "PollingObserver", FakePollingObserver)
    coordinator = SyncCoordinator(get_codeintel_service(tmp_path), sync_callback=lambda _paths: None)

    coordinator._start_observer()

    state = coordinator.status()
    assert state.backend_kind == "polling"
    assert state.state == "degraded"
    assert "native unavailable" in state.degraded_reason
    coordinator.stop()


def test_writer_lease_is_exclusive(tmp_path: Path) -> None:
    path = tmp_path / "writer.lock"
    first = WriterLease(path)
    second = WriterLease(path)
    assert first.acquire()
    assert not second.acquire()
    first.release()
    assert second.acquire()
    second.release()


def test_incremental_sync_re_resolves_reverse_import_closure_without_full_rebuild(
    tmp_path: Path,
    monkeypatch,
) -> None:
    from devcouncil.cli.commands.map import generate_map_artifacts
    import devcouncil.codeintel.sync.incremental as incremental

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "a.py").write_text("def target():\n    return 1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text(
        "from a import target\n\ndef caller():\n    return target()\n",
        encoding="utf-8",
    )
    for index in range(8):
        (tmp_path / f"filler_{index}.py").write_text(
            f"VALUE_{index} = {index}\n",
            encoding="utf-8",
        )
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)
    first_generation = service.store.current_generation()
    (tmp_path / "a.py").write_text("def target():\n    return 2\n", encoding="utf-8")

    monkeypatch.setattr(
        incremental,
        "refresh_map_for_paths",
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("unexpected full rebuild")),
    )
    graph = sync_affected_paths(service, ["a.py"])

    assert service.store.current_generation() == first_generation + 1  # type: ignore[operator]
    assert graph.meta["resolution_scope"] == "affected"
    assert graph.meta["affected_paths"] == ["a.py", "b.py"]
    assert any(
        edge.source.endswith("::caller")
        and edge.target.endswith("::target")
        and edge.kind == "calls"
        for edge in graph.edges
    )


def test_incremental_dead_confidence_and_repo_map_match_token_scan(
    tmp_path: Path,
    monkeypatch,
) -> None:
    import json

    from devcouncil.cli.commands.map import generate_map_artifacts
    import devcouncil.codeintel.sync.incremental as incremental

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    for index in range(8):
        (tmp_path / f"filler_{index}.py").write_text(
            f"VALUE_{index} = {index}\n",
            encoding="utf-8",
        )
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)
    (tmp_path / "app.py").write_text(
        "def main():\n    return 1\n\ndef newly_dead():\n    return 2\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        incremental,
        "_token_scan_dead",
        lambda *args, **kwargs: ([], [], set()),
    )

    graph = sync_affected_paths(service, ["app.py"])

    candidate = next(entry for entry in graph.dead_code if entry.id.endswith("::newly_dead"))
    assert candidate.confidence.value == "ambiguous"
    repo_map = json.loads(
        (tmp_path / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8")
    )
    assert not any("newly_dead" in value for value in repo_map["dead_symbol_candidates"])


def test_incremental_liveness_reliability_and_deleted_map_filter_match_full_build(
    tmp_path: Path,
) -> None:
    import json

    from devcouncil.cli.commands.map import generate_map_artifacts

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "orphan.py").write_text(
        "def helper():\n    return 1\n",
        encoding="utf-8",
    )
    for index in range(8):
        (tmp_path / f"filler_{index}.py").write_text(
            f"VALUE_{index} = {index}\n",
            encoding="utf-8",
        )
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)
    (tmp_path / "orphan.py").unlink()

    graph = sync_affected_paths(service, ["orphan.py"])
    repo_map = json.loads(
        (tmp_path / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8")
    )

    assert graph.meta["liveness_unreachable_unreliable"] is True
    assert repo_map["liveness_unreachable_unreliable"] is True
    assert not any(
        entry["path"] == "orphan.py" for entry in repo_map["files"]
    )
    assert "orphan.py" not in repo_map["unreachable_files"]


def test_incremental_framework_alias_and_abstract_dispatch_match_clean_build(
    tmp_path: Path,
) -> None:
    from devcouncil.cli.commands.map import generate_map_artifacts
    from devcouncil.indexing.graph.build import build_code_graph

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "package.json").write_text(
        '{"name":"app","main":"src/index.ts"}\n',
        encoding="utf-8",
    )
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "handlers.ts").write_text(
        "export function handler() { return 1; }\n"
        "export class Service {}\n",
        encoding="utf-8",
    )
    (tmp_path / "src" / "index.ts").write_text(
        "import { handler as importedHandler, Service as ImportedService }"
        " from './handlers';\n"
        "export function main() { return 0; }\n",
        encoding="utf-8",
    )
    for index in range(8):
        (tmp_path / "src" / f"filler_{index}.ts").write_text(
            f"export const VALUE_{index} = {index};\n",
            encoding="utf-8",
        )
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)
    (tmp_path / "src" / "index.ts").write_text(
        "import { handler as importedHandler, Service as ImportedService }"
        " from './handlers';\n"
        "const callback = importedHandler;\n"
        "const token = ImportedService;\n"
        "app.get('/items', callback);\n"
        "bus.on('ready', callback);\n"
        "container.bind(token);\n"
        "callback();\n"
        "export function main() { return 0; }\n",
        encoding="utf-8",
    )

    incremental = sync_affected_paths(service, ["src/index.ts"])
    clean = build_code_graph(tmp_path)
    semantic_kinds = {
        "registers",
        "routes_to",
        "listens",
        "provides",
        "calls",
    }

    def semantic_edges(graph):
        return {
            (edge.source, edge.target, edge.kind)
            for edge in graph.edges
            if edge.kind in semantic_kinds
        }

    assert semantic_edges(incremental) == semantic_edges(clean)
    assert {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in incremental.dead_code
    } == {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in clean.dead_code
    }


def test_one_file_sync_avoids_repository_liveness_scans_and_persists_dead_cascade(
    tmp_path: Path,
    monkeypatch,
) -> None:
    from devcouncil.cli.commands.map import generate_map_artifacts
    from devcouncil.indexing.graph.build import build_code_graph
    import devcouncil.codeintel.sync.incremental as incremental
    import devcouncil.indexing.graph.liveness as graph_liveness
    import devcouncil.indexing.wiring as wiring
    from devcouncil.indexing.repo_mapper import RepoMapper

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname="fixture"\nversion="0.0.0"\n'
        '[project.scripts]\nfixture="caller:caller"\n',
        encoding="utf-8",
    )
    (tmp_path / "target.py").write_text(
        "def target():\n    return 1\n",
        encoding="utf-8",
    )
    (tmp_path / "caller.py").write_text(
        "from target import target\n\ndef caller():\n    return target()\n",
        encoding="utf-8",
    )
    for index in range(8):
        (tmp_path / f"filler_{index}.py").write_text(
            f"VALUE_{index} = {index}\n",
            encoding="utf-8",
        )
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)
    (tmp_path / "caller.py").write_text(
        "def caller():\n    return 0\n",
        encoding="utf-8",
    )
    clean = build_code_graph(tmp_path)

    def unexpected(*_args, **_kwargs):
        raise AssertionError("unexpected repository-wide liveness scan")

    monkeypatch.setattr(incremental, "refresh_map_for_paths", unexpected)
    monkeypatch.setattr(graph_liveness, "file_liveness", unexpected)
    monkeypatch.setattr(wiring, "build_dynamic_import_index", unexpected)
    monkeypatch.setattr(RepoMapper, "_dead_symbol_candidates", unexpected)

    graph = sync_affected_paths(service, ["caller.py"])

    assert "target.py" in graph.meta["liveness_changed_paths"]
    assert {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in graph.dead_code
    } == {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in clean.dead_code
    }
    assert service.store.last_write_stats["node_payloads_written"] < len(graph.nodes)


def test_incremental_create_rename_delete_matches_clean_rebuild(tmp_path: Path) -> None:
    from devcouncil.cli.commands.map import generate_map_artifacts
    from devcouncil.indexing.graph.build import build_code_graph

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text(
        "def main():\n    return 0\n",
        encoding="utf-8",
    )
    for index in range(10):
        (tmp_path / f"filler_{index}.py").write_text(
            f"VALUE_{index} = {index}\n",
            encoding="utf-8",
        )
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)

    def signature(graph):
        return (
            {
                (node.id, node.kind.value, node.path, node.line, node.end_line)
                for node in graph.nodes
            },
            {
                (edge.source, edge.target, edge.kind, edge.confidence.value)
                for edge in graph.edges
            },
            {
                (entry.id, entry.confidence.value, entry.reason)
                for entry in graph.dead_code
            },
        )

    (tmp_path / "helper.py").write_text(
        "def helper():\n    return 1\n",
        encoding="utf-8",
    )
    (tmp_path / "app.py").write_text(
        "from helper import helper\n\ndef main():\n    return helper()\n",
        encoding="utf-8",
    )
    created = sync_affected_paths(service, ["helper.py", "app.py"])
    assert signature(created) == signature(build_code_graph(tmp_path))

    (tmp_path / "helper.py").rename(tmp_path / "renamed.py")
    (tmp_path / "app.py").write_text(
        "from renamed import helper\n\ndef main():\n    return helper()\n",
        encoding="utf-8",
    )
    renamed = sync_affected_paths(service, ["helper.py", "renamed.py", "app.py"])
    assert signature(renamed) == signature(build_code_graph(tmp_path))
    assert service.store.aliases()

    (tmp_path / "renamed.py").unlink()
    (tmp_path / "app.py").write_text(
        "def main():\n    return 0\n",
        encoding="utf-8",
    )
    deleted = sync_affected_paths(service, ["renamed.py", "app.py"])
    assert signature(deleted) == signature(build_code_graph(tmp_path))
