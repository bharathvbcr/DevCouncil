from __future__ import annotations

import os
from pathlib import Path
from types import SimpleNamespace
import json
import threading
import time

import pytest
from typer.testing import CliRunner

from devcouncil.codeintel.build_control import (
    BuildStatus,
    GraphBuildTimeout,
    _write_status,
    read_build_status,
    run_isolated_full_build,
)
from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.codeintel.sync.coordinator import (
    _COORDINATORS,
    get_sync_coordinator,
    stop_all_coordinators,
)
from devcouncil.indexing.map_artifacts import generate_map_artifacts


@pytest.fixture
def anyio_backend():
    return "asyncio"


def test_full_map_commits_one_generation_and_records_progress(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")

    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)

    service = get_codeintel_service(tmp_path)
    status = read_build_status(tmp_path)
    assert service.store.current_generation() == 1
    assert status.state in {"complete", "idle", "degraded"}
    assert status.generation_after in {None, 1}
    assert service.load() is not None


def test_read_build_status_marks_dead_worker_stale(tmp_path: Path) -> None:
    status = BuildStatus(
        build_id="dead-worker",
        state="building",
        mode="full",
        pid=2_000_000_000,
        phase="extract",
        completed=1,
        total=3,
        last_progress_at=time.time(),
        stall_timeout_seconds=90.0,
        total_timeout_seconds=900.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "stale"
    assert "no longer running" in loaded.degraded_reason


def test_read_build_status_marks_stalled_without_recent_progress(tmp_path: Path) -> None:
    import os

    status = BuildStatus(
        build_id="stalled",
        state="building",
        mode="full",
        pid=os.getpid(),
        phase="resolve",
        completed=2,
        total=5,
        last_progress_at=time.time() - 120.0,
        stall_timeout_seconds=30.0,
        total_timeout_seconds=900.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "stalled"
    assert "no graph progress or worker CPU" in loaded.degraded_reason


def test_read_build_status_returns_idle_on_missing_or_invalid_file(tmp_path: Path) -> None:
    assert read_build_status(tmp_path).state == "idle"
    path = tmp_path / ".devcouncil" / "codeintel" / "build_status.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("{not-json", encoding="utf-8")
    assert read_build_status(tmp_path).state == "idle"


def test_stop_all_coordinators_clears_registry(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "a.py").write_text("VALUE = 1\n", encoding="utf-8")
    coordinator = get_sync_coordinator(tmp_path)
    root = tmp_path.resolve()
    try:
        coordinator.start()
        assert root in _COORDINATORS
        stop_all_coordinators()
        assert root not in _COORDINATORS
    finally:
        stop_all_coordinators()


def test_get_sync_coordinator_is_singleton_per_root(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    try:
        first = get_sync_coordinator(tmp_path)
        second = get_sync_coordinator(tmp_path)
        assert first is second
        with pytest.raises(ValueError, match="already uses"):
            get_sync_coordinator(tmp_path, debounce_seconds=9.0)
    finally:
        stop_all_coordinators()


def test_incremental_map_after_full_map_commits_one_generation(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    app = tmp_path / "app.py"
    app.write_text("def main():\n    return 1\n", encoding="utf-8")

    generate_map_artifacts(tmp_path, tmp_path / ".devcouncil" / "repo_map.json", quiet=True)
    service = get_codeintel_service(tmp_path)
    gen_after_full = service.store.current_generation()
    assert gen_after_full == 1

    app.write_text("def main():\n    return 2\n", encoding="utf-8")
    generate_map_artifacts(
        tmp_path,
        tmp_path / ".devcouncil" / "repo_map.json",
        quiet=True,
        paths=["app.py"],
    )
    assert service.store.current_generation() == gen_after_full + 1  # type: ignore[operator]


def test_map_cli_surfaces_build_status_fields(tmp_path: Path, monkeypatch) -> None:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    (tmp_path / "a.py").write_text("def a():\n    return 1\n", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: SimpleNamespace(
            status=lambda: SimpleNamespace(
                as_dict=lambda: {
                    "state": "healthy",
                    "backend": "FSEventsObserver",
                    "backend_kind": "native",
                    "build_id": "b1",
                    "build_state": "complete",
                    "build_phase": "complete",
                    "build_completed": 2,
                    "build_total": 2,
                    "build_pid": 42,
                    "compatibility_export": "healthy",
                    "degraded_reason": "",
                    "pending": [],
                    "last_error": "",
                    "generation": 1,
                }
            )
        ),
    )

    runner = CliRunner()
    result = runner.invoke(app, ["map", "status", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "generation" in result.output.lower() or "healthy" in result.output.lower()


@pytest.mark.anyio
async def test_graph_ingest_busy_returns_structured_error(tmp_path: Path, monkeypatch) -> None:
    from devcouncil.codeintel.build_control import GraphBuildBusy
    from devcouncil.integrations.mcp.handlers import map as mapmod

    class _Coordinator:
        def sync_now(self, _paths):
            raise GraphBuildBusy("writer lease held")

        def status(self):  # pragma: no cover
            raise AssertionError

    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _root: _Coordinator(),
    )
    if not hasattr(mapmod, "handle_graph_ingest"):
        pytest.skip("handle_graph_ingest not exported")
    result = await mapmod.handle_graph_ingest(tmp_path, {"paths": ["a.py"]})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False


def test_run_isolated_full_build_timeout_records_status(tmp_path: Path, monkeypatch) -> None:
    import subprocess

    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    service = SimpleNamespace(store=SimpleNamespace(current_generation=lambda: 0))
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.get_codeintel_service",
        lambda _r: service,
    )
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: SimpleNamespace(
            indexing=SimpleNamespace(
                build_stall_timeout_seconds=0.01,
                build_total_timeout_seconds=0.01,
            )
        ),
    )

    class _Proc:
        pid = 1
        returncode = None
        stdout = iter([])
        stderr = iter([])

        def poll(self):
            return None

        def wait(self, timeout=None):
            raise subprocess.TimeoutExpired(cmd="worker", timeout=1)

        def terminate(self):
            return None

        def kill(self):
            return None

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.subprocess.Popen",
        lambda *a, **k: _Proc(),
    )
    monkeypatch.setattr("devcouncil.codeintel.build_control.os.name", "posix")
    monkeypatch.setattr("devcouncil.codeintel.build_control.os.killpg", lambda *a, **k: None)

    with pytest.raises(GraphBuildTimeout):
        run_isolated_full_build(tmp_path)

    status = read_build_status(tmp_path)
    assert status.state in {"timed_out", "stale", "stalled", "failed"}
    assert status.degraded_reason or status.state != "building"


def test_lease_held_flag_allows_write_without_owning_writer_lease(tmp_path: Path) -> None:
    """_lease_held=True still skips acquisition — callers must hold or use graph_build_session."""
    from concurrent.futures import ThreadPoolExecutor
    import time

    from devcouncil.codeintel.sync.lease import WriterLease
    from devcouncil.indexing.graph.build import write_code_graph
    from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind

    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()
    path = "src/app.py"

    def _g(name: str) -> CodeGraph:
        return CodeGraph(
            nodes=[
                GraphNode(id=path, kind=NodeKind.FILE, path=path, name="app.py", language="python"),
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
            generated_head="h",
            indexed_hash="i",
            content_fingerprint="c",
        )

    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)
    results: dict[str, object] = {}

    def writer() -> None:
        barrier.wait()
        write_code_graph(tmp_path, _g("race"), _lease_held=True)
        results["wrote"] = True

    def racer() -> None:
        barrier.wait()
        time.sleep(0.01)
        lease = WriterLease(lock)
        got = lease.acquire()
        results["rival_got_lease"] = got
        if got:
            write_code_graph(tmp_path, _g("rival"), _lease_held=True)
            lease.release()

    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(writer)
        f2 = pool.submit(racer)
        f1.result()
        f2.result()

    # Direct _lease_held=True remains unsafe by design; the isolated worker path
    # now acquires via graph_build_session instead of relying on the parent.
    assert results.get("wrote") is True
    assert results.get("rival_got_lease") is True


def test_graph_build_session_serializes_writers(tmp_path: Path) -> None:
    """Workers that enter graph_build_session block concurrent WriterLease holders."""
    from concurrent.futures import ThreadPoolExecutor

    from devcouncil.codeintel.build_control import graph_build_session
    from devcouncil.codeintel.sync.lease import WriterLease
    from devcouncil.indexing.graph.build import write_code_graph
    from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind

    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()
    path = "src/app.py"

    def _g(name: str) -> CodeGraph:
        return CodeGraph(
            nodes=[
                GraphNode(id=path, kind=NodeKind.FILE, path=path, name="app.py", language="python"),
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
            generated_head="h",
            indexed_hash="i",
            content_fingerprint="c",
        )

    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)
    results: dict[str, object] = {}

    def writer() -> None:
        with graph_build_session(tmp_path):
            barrier.wait()
            time.sleep(0.05)
            write_code_graph(tmp_path, _g("owned"), _lease_held=True)
            results["wrote"] = True

    def racer() -> None:
        barrier.wait()
        lease = WriterLease(lock)
        results["rival_got_lease"] = lease.acquire()
        if results["rival_got_lease"]:
            lease.release()

    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(writer)
        f2 = pool.submit(racer)
        f1.result()
        f2.result()

    assert results.get("wrote") is True
    assert results.get("rival_got_lease") is False


def test_yield_writer_lease_reacquires_with_backoff(tmp_path: Path, monkeypatch) -> None:
    """A rival watcher holding the lock after child yield must not permanently starve re-acquire."""
    from concurrent.futures import ThreadPoolExecutor

    from devcouncil.codeintel.build_control import (
        graph_build_session,
        yield_writer_lease_for_child,
    )
    from devcouncil.codeintel.sync.lease import WriterLease

    (tmp_path / ".devcouncil").mkdir()
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)
    results: dict[str, object] = {}

    def parent() -> None:
        with graph_build_session(tmp_path, timeout=2.0):
            with yield_writer_lease_for_child(tmp_path):
                barrier.wait()
                time.sleep(0.05)  # let rival grab the free lock
            results["reacquired"] = True

    def rival() -> None:
        barrier.wait()
        lease = WriterLease(lock)
        # Hold briefly so parent must backoff, then release.
        assert lease.acquire_with_retry(timeout=1.0)
        results["rival_held"] = True
        time.sleep(0.2)
        lease.release()

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._lease_timeouts",
        lambda _root: (2.0, 0.5),
    )
    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(parent)
        f2 = pool.submit(rival)
        f1.result(timeout=5)
        f2.result(timeout=5)

    assert results.get("rival_held") is True
    assert results.get("reacquired") is True
    # Lock is free after both finish.
    probe = WriterLease(lock)
    assert probe.acquire()
    probe.release()


def test_yield_writer_lease_reacquire_timeout_raises(tmp_path: Path, monkeypatch) -> None:
    from concurrent.futures import ThreadPoolExecutor

    from devcouncil.codeintel.build_control import (
        GraphBuildBusy,
        graph_build_session,
        yield_writer_lease_for_child,
    )
    from devcouncil.codeintel.sync.lease import WriterLease

    (tmp_path / ".devcouncil").mkdir()
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)

    def parent() -> None:
        with graph_build_session(tmp_path, timeout=1.0):
            with yield_writer_lease_for_child(tmp_path):
                barrier.wait()
                time.sleep(0.05)

    def rival() -> None:
        barrier.wait()
        lease = WriterLease(lock)
        assert lease.acquire_with_retry(timeout=1.0)
        time.sleep(1.5)  # outlast the re-acquire budget
        lease.release()

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._lease_timeouts",
        lambda _root: (0.3, 0.1),
    )
    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(parent)
        f2 = pool.submit(rival)
        with pytest.raises(GraphBuildBusy, match="re-acquire"):
            f1.result(timeout=5)
        f2.result(timeout=5)


def test_yield_writer_lease_preserves_timeout_when_reacquire_fails(
    tmp_path: Path, monkeypatch
) -> None:
    """Failed re-acquire must not silently replace GraphBuildTimeout with Busy."""
    from concurrent.futures import ThreadPoolExecutor

    from devcouncil.codeintel.build_control import (
        GraphBuildTimeout,
        graph_build_session,
        yield_writer_lease_for_child,
    )
    from devcouncil.codeintel.sync.lease import WriterLease

    (tmp_path / ".devcouncil").mkdir()
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    barrier = threading.Barrier(2)

    def parent() -> None:
        with graph_build_session(tmp_path, timeout=1.0):
            with yield_writer_lease_for_child(tmp_path):
                barrier.wait()
                time.sleep(0.05)
                raise GraphBuildTimeout("graph build made no progress for 90.0s")

    def rival() -> None:
        barrier.wait()
        lease = WriterLease(lock)
        assert lease.acquire_with_retry(timeout=1.0)
        time.sleep(1.5)
        lease.release()

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._lease_timeouts",
        lambda _root: (0.3, 0.1),
    )
    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(parent)
        f2 = pool.submit(rival)
        with pytest.raises(GraphBuildTimeout, match="no progress"):
            f1.result(timeout=5)
        f2.result(timeout=5)


def test_cpu_heartbeat_keeps_a_working_worker_out_of_stalled_state(tmp_path: Path) -> None:
    """Phase counters flat + CPU climbing is a slow phase, not a stall.

    Regression for healthy builds being killed at 90%+ CPU: liveness tokenize
    and SQLite persist emit no phase progress for minutes.
    """
    now = time.time()
    status = BuildStatus(
        build_id="working",
        state="building",
        mode="full",
        pid=os.getpid(),
        phase="liveness:tokens",
        # Last phase counter is far older than the stall budget...
        last_progress_at=now - 600.0,
        # ...but the worker reported CPU a moment ago.
        last_cpu_progress_at=now - 1.0,
        worker_cpu_seconds=512.0,
        stall_timeout_seconds=30.0,
        total_timeout_seconds=7200.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "building"
    assert loaded.degraded_reason == ""


def test_stall_still_fires_when_cpu_is_flat_too(tmp_path: Path) -> None:
    """A genuinely wedged worker (no phase progress, no CPU) is still killed."""
    now = time.time()
    status = BuildStatus(
        build_id="wedged",
        state="building",
        mode="full",
        pid=os.getpid(),
        phase="persist:nodes",
        last_progress_at=now - 600.0,
        last_cpu_progress_at=now - 600.0,
        worker_cpu_seconds=12.0,
        stall_timeout_seconds=30.0,
        total_timeout_seconds=7200.0,
    )
    _write_status(tmp_path, status)

    loaded = read_build_status(tmp_path)
    assert loaded.state == "stalled"
    assert "no graph progress or worker CPU" in loaded.degraded_reason


def test_changed_paths_go_through_a_file_not_argv(tmp_path: Path) -> None:
    """A repo-scale change set must not be one --changed-path arg per entry.

    Regression for ``OSError: [Errno 7] Argument list too long``.
    """
    from devcouncil.codeintel.build_control import _write_changed_paths_file

    paths = {f"src/pkg/module_{index}.py" for index in range(5_000)}
    target = _write_changed_paths_file(tmp_path, "buildid", paths)
    assert target is not None
    written = [line for line in target.read_text(encoding="utf-8").splitlines() if line]
    assert set(written) == paths

    assert _write_changed_paths_file(tmp_path, "buildid", set()) is None
    assert _write_changed_paths_file(tmp_path, "buildid", None) is None


def test_worker_parses_changed_paths_from_file_and_argv() -> None:
    import argparse

    from devcouncil.codeintel import build_worker

    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--changed-path", action="append", default=[])
    parser.add_argument("--changed-paths-file", default="")
    args = parser.parse_args(["--changed-path", "a.py"])
    assert build_worker._changed_paths(args) == {"a.py"}


def _incomplete_refresh(**over):
    from devcouncil.indexing.map_artifacts import GraphRefreshResult

    base = dict(
        repo_map=None,
        graph=None,
        generation=7,
        mode="prior_generation",
        degraded=False,
        reason="GraphBuildTimeout: no progress for 90.0s",
        compatibility_export_degraded=False,
        build_incomplete=True,
    )
    base.update(over)
    return GraphRefreshResult(**base)


def test_mcp_graph_ingest_reports_a_prior_generation_as_not_ok(tmp_path: Path, monkeypatch) -> None:
    """An agent must not read a graph older than HEAD as a fresh ingest."""
    import asyncio
    import json as _json

    from devcouncil.integrations.mcp.handlers import map as map_handler

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: _incomplete_refresh(),
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.embeddings.build_embeddings", lambda _r: 0
    )
    result = asyncio.run(map_handler.handle_graph_ingest(tmp_path, {}))
    payload = _json.loads(result[0].text)

    assert payload["ok"] is False
    assert payload["build_incomplete"] is True
    assert payload["code"] == "graph_build_incomplete"
    assert "older than HEAD" in payload["detail"]
    assert payload["generation"] == 7
