from __future__ import annotations

import json
from pathlib import Path
import subprocess
import threading
import time

import pytest

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


def test_two_watchers_serialize_on_writer_lease_and_both_drain(tmp_path: Path) -> None:
    """Two SyncCoordinator writers on one project must serialize and both finish.

    Regression for the multi-watcher race: without bounded lease retry the loser
    stayed ``read_only`` with pending uncleared, and a failed re-acquire could
    stamp a lean map over a healthy generation.
    """
    from concurrent.futures import ThreadPoolExecutor

    (tmp_path / "a.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("VALUE = 2\n", encoding="utf-8")
    service = get_codeintel_service(tmp_path)
    service.persist(
        CodeGraph(
            nodes=[
                GraphNode(
                    id="a.py",
                    kind=NodeKind.FILE,
                    path="a.py",
                    name="a.py",
                    language="python",
                ),
                GraphNode(
                    id="b.py",
                    kind=NodeKind.FILE,
                    path="b.py",
                    name="b.py",
                    language="python",
                ),
            ]
        )
    )

    order: list[str] = []
    lock = threading.Lock()
    active = 0
    max_active = 0

    def make_callback(name: str):
        def _cb(paths: list[str]) -> None:
            nonlocal active, max_active
            with lock:
                active += 1
                max_active = max(max_active, active)
                order.append(f"{name}:start:{','.join(paths)}")
            time.sleep(0.25)  # hold writer.lock long enough that the peer must backoff
            with lock:
                active -= 1
                order.append(f"{name}:done")

        return _cb

    first = SyncCoordinator(
        service,
        sync_callback=make_callback("w1"),
        debounce_seconds=60.0,
        reconcile_seconds=300.0,
    )
    second = SyncCoordinator(
        get_codeintel_service(tmp_path),
        sync_callback=make_callback("w2"),
        debounce_seconds=60.0,
        reconcile_seconds=300.0,
    )
    first.mark_pending("a.py")
    second.mark_pending("b.py")

    results: dict[str, bool] = {}

    def run(label: str, coordinator: SyncCoordinator) -> None:
        results[label] = coordinator.sync_now()

    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(run, "w1", first)
        f2 = pool.submit(run, "w2", second)
        assert f1.result(timeout=20) is None or True
        assert f2.result(timeout=20) is None or True

    assert results == {"w1": True, "w2": True}
    assert first.status().pending == []
    assert second.status().pending == []
    assert max_active == 1, f"writers overlapped: {order}"
    starts = [item for item in order if ":start:" in item]
    dones = [item for item in order if item.endswith(":done")]
    assert len(starts) == 2 and len(dones) == 2
    # Fully serialized: first done precedes second start.
    assert order.index(dones[0]) < order.index(starts[1])


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


def test_index_scope_excludes_nested_vendor_min_js_like_graph_ingestion(
    tmp_path: Path,
) -> None:
    """Watcher scope must match graph exclusion or reconcile loops forever.

    Nested ``assets/vendor/force-graph.min.js`` is not under a root ``vendor/``
    prefix, so prefix-only ignores miss it while ``is_vendored_path`` / graph
    build correctly drop it — leaving the path perpetually "changed".
    """
    vendor = tmp_path / "src" / "devcouncil" / "assets" / "vendor"
    vendor.mkdir(parents=True)
    force_graph = vendor / "force-graph.min.js"
    force_graph.write_text("/* vendor */\n", encoding="utf-8")
    (tmp_path / "src" / "app.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()

    scope = IndexScope(tmp_path)
    rel = "src/devcouncil/assets/vendor/force-graph.min.js"
    assert not scope.includes(rel)
    assert rel not in scope.files()
    assert scope.includes("src/app.py")

    _persist_file(tmp_path, "src/app.py")
    coordinator = SyncCoordinator(
        get_codeintel_service(tmp_path),
        sync_callback=lambda _paths: None,
    )
    assert rel not in coordinator.reconcile()
    assert coordinator.reconcile() == []


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


def test_fsevents_preflight_retries_transient_timeout(tmp_path: Path, monkeypatch) -> None:
    from devcouncil.codeintel.sync import coordinator as coordinator_mod

    calls: list[str] = []

    def flaky_run(*_args, **_kwargs):
        calls.append("run")
        if len(calls) == 1:
            raise subprocess.TimeoutExpired(cmd=_args[0], timeout=5.0)
        return subprocess.CompletedProcess(_args[0], 0, stdout=b"", stderr=b"")

    monkeypatch.setattr(coordinator_mod.subprocess, "run", flaky_run)
    monkeypatch.setattr(coordinator_mod.time, "sleep", lambda _seconds: None)
    assert coordinator_mod._fsevents_preflight(tmp_path) is True
    assert len(calls) == 2


def test_writer_lease_is_exclusive(tmp_path: Path) -> None:
    path = tmp_path / "writer.lock"
    first = WriterLease(path)
    second = WriterLease(path)
    assert first.acquire()
    assert not second.acquire()
    first.release()
    assert second.acquire()
    second.release()


def test_writer_lease_acquire_with_retry_backoff(tmp_path: Path, monkeypatch) -> None:
    path = tmp_path / "writer.lock"
    holder = WriterLease(path)
    assert holder.acquire()
    contender = WriterLease(path)
    sleeps: list[float] = []

    def fake_sleep(seconds: float) -> None:
        sleeps.append(seconds)
        if len(sleeps) == 2:
            holder.release()

    assert contender.acquire_with_retry(
        timeout=1.0, initial_delay=0.05, max_delay=0.2, sleep=fake_sleep
    )
    assert sleeps  # backed off at least once before the holder released
    assert sleeps[0] <= sleeps[-1] or len(sleeps) == 1
    contender.release()


def test_writer_lease_acquire_with_retry_times_out(tmp_path: Path, monkeypatch) -> None:
    path = tmp_path / "writer.lock"
    holder = WriterLease(path)
    assert holder.acquire()
    contender = WriterLease(path)
    monkeypatch.setattr("devcouncil.codeintel.sync.lease.time.sleep", lambda _s: None)
    # Force deadline to expire immediately after the first failed probe.
    monotonic = iter([100.0, 100.0, 101.0])
    monkeypatch.setattr(
        "devcouncil.codeintel.sync.lease.time.monotonic",
        lambda: next(monotonic, 101.0),
    )
    assert contender.acquire_with_retry(timeout=0.5, initial_delay=0.05) is False
    holder.release()


def test_sync_now_retries_busy_lease_then_succeeds(tmp_path: Path, monkeypatch) -> None:
    from concurrent.futures import ThreadPoolExecutor

    from devcouncil.codeintel.sync.lease import WriterLease

    (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
    service = get_codeintel_service(tmp_path)
    calls: list[list[str]] = []
    coordinator = SyncCoordinator(
        service,
        sync_callback=lambda paths: calls.append(list(paths)),
    )
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    holder = WriterLease(lock)
    assert holder.acquire()

    def release_soon() -> None:
        time.sleep(0.15)
        holder.release()

    coordinator.mark_pending("app.py")
    with ThreadPoolExecutor(max_workers=1) as pool:
        pool.submit(release_soon)
        # Use a short sync timeout so the test stays fast; retry still wins.
        monkeypatch.setattr(
            "devcouncil.codeintel.build_control._lease_timeouts",
            lambda _root: (30.0, 2.0),
        )
        assert coordinator.sync_now() is True
    assert calls == [["app.py"]]
    assert coordinator.status().pending == []
    assert coordinator.status().state in {"healthy", "degraded"}


def test_map_artifacts_does_not_lean_on_graph_build_busy(tmp_path: Path, monkeypatch) -> None:
    from devcouncil.codeintel.build_control import GraphBuildBusy
    from devcouncil.indexing import map_artifacts

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")

    def boom(*_a, **_k):
        raise GraphBuildBusy(
            "could not re-acquire the code-intelligence writer lease after isolated build"
        )

    # Patch the module the function imports from (local import inside refresh).
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.run_isolated_full_build",
        boom,
    )
    with pytest.raises(GraphBuildBusy, match="re-acquire"):
        map_artifacts.refresh_map_artifacts(
            tmp_path,
            tmp_path / ".devcouncil" / "repo_map.json",
            quiet=True,
        )
    # Must not stamp a lean degraded map over lease contention.
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    assert not map_path.is_file() or not json.loads(
        map_path.read_text(encoding="utf-8")
    ).get("graph_degraded")


def test_incremental_sync_re_resolves_reverse_import_closure_without_full_rebuild(
    tmp_path: Path,
) -> None:
    from devcouncil.cli.commands.map import generate_map_artifacts

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
) -> None:
    import json

    from devcouncil.cli.commands.map import generate_map_artifacts

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
    graph = sync_affected_paths(service, ["app.py"])

    candidate = next(entry for entry in graph.dead_code if entry.id.endswith("::newly_dead"))
    assert candidate.confidence.value == "extracted"
    assert graph.meta["resolution_scope"] == "full"
    repo_map = json.loads(
        (tmp_path / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8")
    )
    assert any("newly_dead" in value for value in repo_map["dead_symbol_candidates"])


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


def test_resolution_surface_change_uses_full_build_and_preserves_dead_code(
    tmp_path: Path,
) -> None:
    from devcouncil.cli.commands.map import generate_map_artifacts
    from devcouncil.indexing.graph.build import build_code_graph

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

    graph = sync_affected_paths(service, ["caller.py"])

    assert graph.meta["resolution_scope"] == "full"
    assert {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in graph.dead_code
    } == {
        (entry.id, entry.confidence.value, entry.reason)
        for entry in clean.dead_code
    }


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


def test_incremental_prunes_stale_vendored_analysis_shards(tmp_path: Path) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    graph = build_code_graph(tmp_path)
    write_code_graph(tmp_path, graph)
    service = get_codeintel_service(tmp_path)
    shards = service.store.analysis_shards()
    sample = dict(next(iter(shards.values())))
    extraction = dict(sample["extraction"])
    extraction["path"] = "src/vendor/bundle.js"
    sample["extraction"] = extraction
    shards["src/vendor/bundle.js"] = sample
    service.persist(graph, analysis_shards=shards)

    (tmp_path / "app.py").write_text("def main():\n    return 2\n", encoding="utf-8")
    updated = sync_affected_paths(service, ["app.py"])

    assert updated.meta["resolution_scope"] == "affected"
    assert "src/vendor/bundle.js" not in service.store.analysis_shards()
    assert not any(node.path == "src/vendor/bundle.js" for node in updated.nodes)


def test_incremental_removes_orphaned_pathless_semantic_nodes(tmp_path: Path) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    source = tmp_path / "app.js"
    source.write_text(
        "function main() {\n  bus.emit('ready');\n}\n",
        encoding="utf-8",
    )
    graph = build_code_graph(tmp_path)
    write_code_graph(tmp_path, graph)
    assert any(node.id == "event::ready" for node in graph.nodes)

    source.write_text("function main() {\n  return 1;\n}\n", encoding="utf-8")
    updated = sync_affected_paths(get_codeintel_service(tmp_path), ["app.js"])

    assert updated.meta["resolution_scope"] == "affected"
    assert not any(node.id == "event::ready" for node in updated.nodes)


def test_new_global_symbol_collision_falls_back_and_matches_clean_build(tmp_path: Path) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "a.py").write_text("def shared():\n    return 1\n", encoding="utf-8")
    (tmp_path / "caller.py").write_text(
        "def caller():\n    return shared()\n",
        encoding="utf-8",
    )
    write_code_graph(tmp_path, build_code_graph(tmp_path))
    (tmp_path / "b.py").write_text("def shared():\n    return 2\n", encoding="utf-8")

    updated = sync_affected_paths(get_codeintel_service(tmp_path), ["b.py"])
    clean = build_code_graph(tmp_path)
    def edge_signature(value):
        return {
            (edge.source, edge.target, edge.kind, edge.confidence.value)
            for edge in value.edges
        }

    assert updated.meta["resolution_scope"] == "full"
    assert edge_signature(updated) == edge_signature(clean)


def test_incremental_preserves_unchanged_inbound_edges_and_communities(tmp_path: Path) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    changed = tmp_path / "changed.py"
    changed.write_text("def dependency():\n    return 1\n", encoding="utf-8")
    (tmp_path / "target.py").write_text(
        "from changed import dependency\n\ndef target_fn():\n    return dependency()\n",
        encoding="utf-8",
    )
    (tmp_path / "caller.py").write_text(
        "def caller():\n    return target_fn()\n",
        encoding="utf-8",
    )
    write_code_graph(tmp_path, build_code_graph(tmp_path))

    changed.write_text(
        "# implementation-only edit\ndef dependency():\n    return 1\n",
        encoding="utf-8",
    )
    updated = sync_affected_paths(get_codeintel_service(tmp_path), ["changed.py"])
    clean = build_code_graph(tmp_path)

    def payloads(values):
        return sorted(
            json.dumps(value.model_dump(mode="json"), sort_keys=True)
            for value in values
        )

    assert updated.meta["resolution_scope"] == "affected"
    assert {node.id: node.model_dump(mode="json") for node in updated.nodes} == {
        node.id: node.model_dump(mode="json") for node in clean.nodes
    }
    assert payloads(updated.edges) == payloads(clean.edges)
    assert payloads(updated.dead_code) == payloads(clean.dead_code)
    assert updated.unwired_candidates == clean.unwired_candidates
    assert updated.unreachable_files == clean.unreachable_files


def test_incremental_duplicate_aliases_share_symbol_liveness(tmp_path: Path) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    changed = tmp_path / "changed.py"
    changed.write_text("def dependency():\n    return 1\n", encoding="utf-8")
    (tmp_path / "model.py").write_text(
        "class Model:\n"
        "    @property\n"
        "    def stream(self):\n"
        "        return None\n\n"
        "    @stream.setter\n"
        "    def stream(self, value):\n"
        "        pass\n",
        encoding="utf-8",
    )
    write_code_graph(tmp_path, build_code_graph(tmp_path))

    changed.write_text(
        "# implementation-only edit\ndef dependency():\n    return 1\n",
        encoding="utf-8",
    )
    updated = sync_affected_paths(get_codeintel_service(tmp_path), ["changed.py"])
    clean = build_code_graph(tmp_path)

    def dead_payloads(graph):
        return sorted(
            json.dumps(entry.model_dump(mode="json"), sort_keys=True)
            for entry in graph.dead_code
        )

    assert updated.meta["resolution_scope"] == "affected"
    assert dead_payloads(updated) == dead_payloads(clean)


def test_shard_liveness_excludes_nonproduction_entry_roots_from_unwired(
    tmp_path: Path,
) -> None:
    from devcouncil.indexing.graph.liveness import file_liveness_from_shards

    roots, unwired, unreachable, unreliable = file_liveness_from_shards(
        ["script.py"],
        [],
        {"script.py": {"allow_unwired": False}},
        root=tmp_path,
        entry_roots=["script.py"],
        production_entry_roots=[],
    )

    assert roots == []
    assert unwired == []
    assert unreachable == []
    assert unreliable is True


def test_shard_liveness_applies_unreachable_ratio_gate(tmp_path: Path) -> None:
    """Incremental shard liveness fails soft on an unreachable flood.

    Parity with the full build: when static BFS misses most files (dynamic
    imports / routers), ``file_liveness`` suppresses the flood via the density
    gate — the shard path must not resurrect it on the next incremental sync.
    """
    from devcouncil.indexing.graph.liveness import file_liveness_from_shards

    files = ["main.py"] + [f"mod_{i}.py" for i in range(9)]
    shards: dict[str, dict[str, object]] = {
        path: {"allow_unwired": False} for path in files
    }
    roots, _unwired, unreachable, unreliable = file_liveness_from_shards(
        files,
        [],  # no import edges: 9/10 files look unreachable, far past the gate
        shards,
        root=tmp_path,
        entry_roots=["main.py"],
        production_entry_roots=["main.py"],
    )

    assert roots == ["main.py"]
    assert unreachable == []
    assert unreliable is True


def test_incremental_unsharded_config_references_match_full_liveness(
    tmp_path: Path,
) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    changed = tmp_path / "changed.py"
    changed.write_text("def dependency():\n    return 1\n", encoding="utf-8")
    plugin = tmp_path / "plugin"
    plugin.mkdir()
    (plugin / "hatch_build.py").write_text(
        "def hook():\n    return 1\n",
        encoding="utf-8",
    )
    (plugin / "pyproject.toml").write_text(
        "[tool.hatch.build.hooks.custom]\npath = 'hatch_build.py'\n",
        encoding="utf-8",
    )
    write_code_graph(tmp_path, build_code_graph(tmp_path))

    changed.write_text(
        "# implementation-only edit\ndef dependency():\n    return 1\n",
        encoding="utf-8",
    )
    updated = sync_affected_paths(get_codeintel_service(tmp_path), ["changed.py"])
    clean = build_code_graph(tmp_path)

    assert updated.meta["resolution_scope"] == "affected"
    assert updated.unwired_candidates == clean.unwired_candidates

def test_incremental_persists_global_community_relabels(
    tmp_path: Path,
    monkeypatch,
) -> None:
    import devcouncil.indexing.graph.intel as graph_intel
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    changed = tmp_path / "a.py"
    changed.write_text("def a():\n    return 1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("def b():\n    return 2\n", encoding="utf-8")
    write_code_graph(tmp_path, build_code_graph(tmp_path))
    original = graph_intel.enrich_graph_intel

    def relabel(graph, *, root, seed=0):
        result = original(graph, root=root, seed=seed)
        for node in graph.nodes:
            if node.path == "b.py":
                node.community = "forced-global-relabel"
        return result

    monkeypatch.setattr(graph_intel, "enrich_graph_intel", relabel)
    changed.write_text(
        "# implementation-only edit\ndef a():\n    return 1\n",
        encoding="utf-8",
    )
    service = get_codeintel_service(tmp_path)
    updated = sync_affected_paths(service, ["a.py"])
    loaded = service.load()

    assert updated.meta["resolution_scope"] == "affected"
    assert "b.py" in updated.meta["community_changed_paths"]
    assert loaded is not None
    assert {
        node.community for node in loaded.nodes if node.path == "b.py"
    } == {"forced-global-relabel"}


def test_incremental_sync_keeps_go_same_package_callee_wired(tmp_path: Path) -> None:
    """The call-edge liveness projection must survive incremental refreshes."""
    import pytest

    try:
        from devcouncil.codeintel.languages import grammar_status

        go_missing = any(
            row.get("missing_grammars")
            for row in grammar_status().get("languages", [])
            if row.get("language") == "Go"
        )
    except Exception:
        go_missing = True
    if go_missing:
        pytest.skip("go grammar not installed")

    from devcouncil.cli.commands.map import generate_map_artifacts

    (tmp_path / ".devcouncil").mkdir()
    cmd = tmp_path / "cmd" / "server"
    cmd.mkdir(parents=True)
    (cmd / "main.go").write_text(
        "package main\n\nfunc main() {\n\thandle()\n}\n", encoding="utf-8"
    )
    (cmd / "handlers.go").write_text("package main\n\nfunc handle() {}\n", encoding="utf-8")
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
    )
    service = get_codeintel_service(tmp_path)
    full_graph = service.load()
    assert full_graph is not None
    assert "cmd/server/handlers.go" not in full_graph.unwired_candidates

    (cmd / "handlers.go").write_text(
        "package main\n\nfunc handle() {\n\t_ = 1\n}\n", encoding="utf-8"
    )
    graph = sync_affected_paths(service, ["cmd/server/handlers.go"])
    # Body-only edit keeps the resolution surface stable → true incremental path.
    assert graph.meta.get("incremental") is True
    assert "cmd/server/handlers.go" not in graph.unwired_candidates
    assert "cmd/server/handlers.go" not in graph.unreachable_files


_REPO_ROOT = Path(__file__).resolve().parents[2]
_FORCE_GRAPH = "src/devcouncil/assets/vendor/force-graph.min.js"


def test_live_repo_force_graph_excluded_from_watcher_scope() -> None:
    from devcouncil.indexing.wiring import is_vendored_path

    assert is_vendored_path(_FORCE_GRAPH)
    scope = IndexScope(_REPO_ROOT)
    assert not scope.includes(_FORCE_GRAPH)
    assert _FORCE_GRAPH not in scope.files()


def test_live_repo_reconcile_does_not_flag_force_graph() -> None:
    service = get_codeintel_service(_REPO_ROOT)
    coordinator = SyncCoordinator(service, sync_callback=lambda _paths: None)
    assert _FORCE_GRAPH not in coordinator.reconcile()


def test_full_refresh_then_single_edit_parity_with_vendor_present(tmp_path: Path) -> None:
    from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

    (tmp_path / ".devcouncil").mkdir()
    helper = tmp_path / "helper.py"
    app = tmp_path / "app.py"
    helper.write_text("def helper():\n    return 1\n", encoding="utf-8")
    app.write_text(
        "from helper import helper\n\ndef main():\n    return helper()\n",
        encoding="utf-8",
    )
    for index in range(6):
        (tmp_path / f"filler_{index}.py").write_text(
            f"VALUE_{index} = {index}\n",
            encoding="utf-8",
        )
    vendor = tmp_path / "assets" / "vendor"
    vendor.mkdir(parents=True)
    (vendor / "force-graph.min.js").write_text("/* vendor */\n", encoding="utf-8")

    write_code_graph(tmp_path, build_code_graph(tmp_path))
    service = get_codeintel_service(tmp_path)
    before = service.store.current_generation()

    app.write_text(
        "from helper import helper\n\ndef main():\n    return helper() + 1\n",
        encoding="utf-8",
    )
    incremental = sync_affected_paths(service, ["app.py"])
    full = build_code_graph(tmp_path)

    def signature(graph):
        return (
            sorted((n.id, n.kind.value, n.path, n.name) for n in graph.nodes),
            sorted((e.source, e.target, e.kind) for e in graph.edges),
        )

    assert service.store.current_generation() == before + 1  # type: ignore[operator]
    assert signature(incremental) == signature(full)
    assert not any("vendor" in (n.path or "") for n in incremental.nodes)
    coordinator = SyncCoordinator(service, sync_callback=lambda _paths: None)
    assert "assets/vendor/force-graph.min.js" not in coordinator.reconcile()
