"""The writer lease, and the kernel seam that is the only map/graph writer.

The watcher (``SyncCoordinator``), the watch scope (``IndexScope``) and the
Python incremental engine (``sync_affected_paths``) were retired with the
Python graph engine, and every test that exercised them went with them: they
asserted the behaviour of a second writer of ``repo_map.json`` /
``code_graph.json`` that no longer exists.
"""

from __future__ import annotations

import json
import subprocess
import time
from pathlib import Path

import pytest

from devcouncil.codeintel import sync as sync_package
from devcouncil.codeintel.sync.lease import WriterLease


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


def _have_kernel() -> bool:
    from devcouncil.devmap_engine import DevMapEngineError, find_engine_binary

    try:
        find_engine_binary()
    except DevMapEngineError:
        return False
    return True


requires_kernel = pytest.mark.skipif(
    not _have_kernel(), reason="devmap kernel not built (cargo build --release -p devmap-cli)"
)


def test_map_artifacts_propagates_a_kernel_failure_without_writing_a_map(
    tmp_path: Path, monkeypatch
) -> None:
    """A kernel that cannot build fails closed: the error propagates, nothing is written.

    This is the kernel-era form of the retired lease-contention case. The
    Python engine could answer a busy writer lease by stamping a lean,
    ``graph_degraded`` map over a healthy store; the kernel is the only writer
    now, so a failed build must leave no artifact behind for ``--if-stale`` to
    mistake for a fresh one.
    """
    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.indexing import map_artifacts

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")

    def boom(*_a, **_k):
        raise DevMapEngineError(
            "devmap build failed (exit 1): the store is locked by devmap pid 4242"
        )

    monkeypatch.setattr("devcouncil.devmap_engine.build_map_result", boom)
    with pytest.raises(DevMapEngineError, match="locked"):
        map_artifacts.refresh_map_artifacts(
            tmp_path,
            tmp_path / ".devcouncil" / "repo_map.json",
            quiet=True,
        )
    assert not (tmp_path / ".devcouncil" / "repo_map.json").exists()
    assert not (tmp_path / ".devcouncil" / "graph" / "code_graph.json").exists()


def test_map_artifacts_keeps_the_prior_artifacts_when_the_kernel_times_out(
    tmp_path: Path, monkeypatch
) -> None:
    """A timed-out build leaves the previous map and graph byte-identical.

    The kernel commits a generation in one transaction, so a timeout leaves
    the store on the prior generation. The seam must leave the prior artifacts
    alone too: their fingerprints describe the tree the prior generation was
    built from, and restamping them from today's tree would make a stale map
    read fresh. (The Python engine used to do exactly that when it recovered
    from a timeout without a prior generation.)
    """
    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.indexing import map_artifacts

    devcouncil_dir = tmp_path / ".devcouncil"
    (devcouncil_dir / "graph").mkdir(parents=True)
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    map_path = devcouncil_dir / "repo_map.json"
    graph_path = devcouncil_dir / "graph" / "code_graph.json"
    map_path.write_text(
        json.dumps(
            {
                "map_engine": "devmap-rust",
                "generated_head": "prior-head",
                "indexed_hash": "prior-hash",
                "content_fingerprint": "prior-fingerprint",
                "files": [],
            }
        ),
        encoding="utf-8",
    )
    graph_path.write_text(
        json.dumps({"meta": {"map_engine": "devmap-rust"}, "nodes": [], "edges": []}),
        encoding="utf-8",
    )
    before = (map_path.read_bytes(), graph_path.read_bytes())

    def boom(*_a, **_k):
        raise DevMapEngineError("devmap timed out after 900s: build")

    monkeypatch.setattr("devcouncil.devmap_engine.build_map_result", boom)
    with pytest.raises(DevMapEngineError, match="timed out"):
        map_artifacts.refresh_map_artifacts(tmp_path, map_path, quiet=True, full=True)

    assert (map_path.read_bytes(), graph_path.read_bytes()) == before


@requires_kernel
def test_map_artifacts_reuses_generation_when_nothing_changed(tmp_path: Path) -> None:
    """``dev map`` on an unchanged tree must not write a new generation.

    The kernel's unchanged-skip path is what makes the post-tool-use hook and
    ``--if-stale`` cheap; a second build of the same tree has to come back on
    the generation the first one committed.
    """
    from devcouncil.indexing import map_artifacts

    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / "app.py").write_text("def main():\n    return 1\n", encoding="utf-8")
    map_path = tmp_path / ".devcouncil" / "repo_map.json"

    first = map_artifacts.refresh_map_artifacts(tmp_path, map_path, quiet=True)
    second = map_artifacts.refresh_map_artifacts(tmp_path, map_path, quiet=True)

    assert first.generation is not None
    assert second.generation == first.generation
    assert second.degraded is False
    assert second.mode == map_artifacts.MAP_ENGINE
    assert json.loads(map_path.read_text(encoding="utf-8"))["map_engine"] == "devmap-rust"


# --- The kernel is the only writer -------------------------------------------


def test_mcp_lifespan_warms_the_kernel_daemon_and_starts_no_python_watcher(
    tmp_path: Path, monkeypatch
) -> None:
    """MCP auto-sync is a kernel warm-up, not a second writer.

    ``_lifespan`` used to start a ``SyncCoordinator``, which re-extracted with
    the Python engine and rewrote ``repo_map.json`` / ``code_graph.json`` on
    every edit for the life of the MCP process. The kernel daemon watches,
    drains and retires itself, so all the lifespan owes it is one status call.
    """
    import asyncio
    from types import SimpleNamespace

    from devcouncil.integrations.mcp import server as mcp_server

    (tmp_path / ".devcouncil").mkdir()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _root: SimpleNamespace(
            code_intelligence=SimpleNamespace(enabled=True, auto_sync=True)
        ),
    )

    warmed: list[Path] = []

    class _Client:
        def __init__(self, root, *_a, **_k):
            self.root = Path(root)

        def status(self):
            warmed.append(self.root)
            return SimpleNamespace(generation_id=1, pending_count=0, is_fresh=True)

    monkeypatch.setattr("devcouncil.devmap_client.DevMapClient", _Client)

    async def _enter() -> dict:
        async with mcp_server._lifespan(None) as context:
            return context

    context = asyncio.run(_enter())
    # The warm-up runs on its own thread; give it a bounded moment to land.
    for _ in range(200):
        if warmed:
            break
        time.sleep(0.01)

    assert warmed == [tmp_path.resolve()], "the lifespan must warm the kernel daemon"
    assert context == {"codeintel": None}
    assert not hasattr(sync_package, "get_sync_coordinator")
    assert not hasattr(sync_package, "SyncCoordinator")


def test_mcp_sync_without_a_kernel_reports_engine_unavailable(
    tmp_path: Path, monkeypatch
) -> None:
    """``devcouncil_code_sync`` must never hand off to a Python engine.

    It used to fall back to ``SyncCoordinator.sync_now`` whenever the daemon
    could not be reached — a second engine answering, writing both artifacts,
    with nothing in the payload saying which one had run.
    """
    import asyncio

    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.integrations.mcp.handlers import codeintel as mcp_codeintel

    monkeypatch.setattr(mcp_codeintel, "try_connect", lambda _root: None)

    def _unavailable(*_a, **_k):
        raise DevMapEngineError("no devmap binary supports this map engine")

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _unavailable
    )

    result = asyncio.run(mcp_codeintel._sync(tmp_path, {"paths": ["app.py"]}))
    payload = json.loads(result[0].text)

    assert payload["ok"] is False
    assert payload["code"] == "engine_unavailable"
    assert "no devmap binary" in payload["error"]
    assert payload["reconciled"] == ["app.py"]


def test_query_envelope_reports_kernel_freshness_not_a_python_watcher(
    tmp_path: Path, monkeypatch
) -> None:
    """The envelope's ``sync`` block answers for the kernel's store.

    It used to report the Python ``SyncCoordinator``'s state — a watcher over a
    store the kernel does not write. An unreachable kernel is now reported as
    ``unavailable`` with the reason, never as a second store's healthy verdict.

    Retargeted from ``CodeIntelQueryEngine._envelope`` to
    ``handlers.codeintel._client_envelope``: the Python engine was deleted with
    the rest of the query surface, and the MCP envelope is the only remaining
    place this block is built.
    """
    from types import SimpleNamespace

    from devcouncil.devmap_client import DevMapClientError
    from devcouncil.integrations.mcp.handlers import codeintel as handlers

    class _Fresh:
        def status(self):
            return SimpleNamespace(
                generation_id=9,
                pending_count=0,
                node_count=2,
                edge_count=1,
                is_fresh=True,
                degraded_reason=None,
                quarantined_count=0,
                raw={"schema_version": 12, "analyzer_version": "devmap"},
            )

    envelope = handlers._client_envelope(tmp_path, _Fresh(), {}, operation="search")
    assert envelope["sync"] == {
        "state": "fresh",
        "pending": 0,
        "fresh": True,
        "degraded_reason": None,
    }
    assert envelope["generation"] == 9

    class _Broken:
        def search(self, query, limit=2000, semantic=False):
            raise DevMapClientError("devmap store is missing; run `dev map`")

        def status(self):
            raise DevMapClientError("devmap store is missing; run `dev map`")

    monkeypatch.setattr(handlers, "try_connect", lambda _root: _Broken())
    degraded = handlers._search_via_client(tmp_path, "anything", 10)
    # Not a zero-match answer: a kernel that could not be asked must not read
    # like one that was asked and found nothing.
    assert degraded["ok"] is False
    assert degraded["matches"] == []
    assert "devmap store is missing" in degraded["resolution"]["Unavailable"]["reason"]


def test_hook_map_refresh_defers_loudly_when_the_kernel_cannot_build(
    tmp_path: Path, monkeypatch, caplog
) -> None:
    """A hook must not fail the tool call, and must not swallow the reason.

    The post-tool-use refresher used to call ``refresh_map_for_paths`` inside a
    blanket ``except Exception: logger.debug(...)``, so a kernel that could not
    run left no record at any level a user sees and the batch it was holding
    was dropped. The kernel seam raises ``DevMapEngineError``; the hook logs it
    at WARNING and re-queues the batch for the next hook.
    """
    import logging

    from devcouncil.cli.commands.hook import _maybe_refresh_map
    from devcouncil.devmap_engine import DevMapEngineError

    monkeypatch.setattr("devcouncil.cli.commands.hook.MAP_REFRESH_DEBOUNCE_S", 0.0)
    calls: list[dict] = []

    def _unavailable(root, output, *_a, **kwargs):
        calls.append({"root": Path(root), "output": Path(output), "kwargs": dict(kwargs)})
        raise DevMapEngineError("another devmap writer (pid 4242) holds the store")

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _unavailable
    )

    payload = json.dumps({"tool_name": "Write", "file_path": "src/app.py"})
    with caplog.at_level(logging.WARNING, logger="devcouncil.cli.commands.hook"):
        _maybe_refresh_map(tmp_path, payload)

    assert calls, "the hook must refresh through the kernel seam"
    # No `paths`, deliberately. The seam used to accept one and `del` it on
    # arrival — `build_map` has no path list and the kernel decides for itself
    # which files a build revisits — so every caller computing one was paying
    # for precision the function could not use. This asserts the plumbing stays
    # gone; the queue assertion below is where knowing *what* changed still
    # earns its keep, because that decides whether to build at all.
    assert "paths" not in calls[0]["kwargs"], (
        "the dead `paths` plumbing was reintroduced: refresh_map_artifacts discards it, "
        f"so passing {calls[0]['kwargs'].get('paths')!r} buys nothing and reads as precision"
    )
    assert calls[0]["output"] == tmp_path / ".devcouncil" / "repo_map.json"
    assert any("map refresh deferred" in record.message for record in caplog.records)
    assert len(calls) == 1, "a kernel that just failed must not be retried in the same hook"
    queue = tmp_path / ".devcouncil" / "cache" / "map_refresh_queue.json"
    assert json.loads(queue.read_text(encoding="utf-8"))["paths"] == ["src/app.py"], (
        "an undelivered batch must go back on the queue, not be dropped"
    )
