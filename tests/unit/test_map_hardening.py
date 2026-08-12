"""Hardening regressions for the dev map build pipeline.

Each test here encodes a defect observed in production on 2026-08-11:
- ``_prune`` deleting FTS5 rows via the UNINDEXED ``generation_id`` column ran a
  single statement for 2+ hours at 100% CPU (full vtab scan + inverted-index
  churn inside one ever-growing WAL transaction).
- The supervised build worker had no self-enforced deadline: killing the parent
  ``dev map`` left an orphaned session-leader worker grinding forever while
  holding ``writer.lock``.
- ``_terminate_worker`` skipped the group SIGKILL when the leader was already
  dead, leaking SIGTERM-ignoring stragglers (multiprocessing resource trackers).
- ``dev map dead`` reported dead-code results from an index frozen at a commit
  five days behind HEAD with no staleness signal at all.
- ``changed-*.txt`` handoff files accumulated forever when the supervisor died.
"""

from __future__ import annotations

import os
import signal
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import pytest
from typer.testing import CliRunner

from devcouncil.codeintel.store import CodeIntelStore
from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind


def _graph(name: str = "main", *, path: str = "src/app.py", head: str = "abc") -> CodeGraph:
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
        generated_head=head,
        indexed_hash="files",
        content_fingerprint="content",
        meta={"fixture": True},
    )


def _git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def _init_repo_with_commit(root: Path) -> str:
    _git(root, "init", "-q")
    _git(root, "config", "user.email", "test@test")
    _git(root, "config", "user.name", "test")
    (root / "file.txt").write_text("hello\n", encoding="utf-8")
    _git(root, "add", ".")
    _git(root, "commit", "-qm", "initial")
    return _git(root, "rev-parse", "HEAD")


# ---------------------------------------------------------------------------
# 1. FTS prune must never delete by the UNINDEXED generation_id column.
# ---------------------------------------------------------------------------


def test_prune_avoids_unindexed_fts_delete_scan(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    for name in ("first", "second", "third"):
        store.save_graph(_graph(name))

    statements: list[str] = []
    conn = sqlite3.connect(store.path)
    try:
        conn.row_factory = sqlite3.Row
        conn.set_trace_callback(statements.append)
        conn.execute("BEGIN IMMEDIATE")
        CodeIntelStore._prune(conn, keep=1, current=3)
        conn.commit()

        bad = [
            stmt
            for stmt in statements
            if "DELETE" in stmt.upper()
            and "NODES_FTS" in stmt.upper()
            and "GENERATION_ID" in stmt.upper()
        ]
        assert not bad, (
            "prune still deletes FTS rows via the UNINDEXED generation_id "
            f"column (full-scan + index churn): {bad}"
        )

        gens = {
            int(row[0])
            for row in conn.execute("SELECT DISTINCT generation_id FROM nodes_fts")
        }
        assert gens == {3}, f"FTS should hold only the kept generation, got {gens}"
        committed = {
            int(row[0]) for row in conn.execute("SELECT id FROM generations")
        }
        assert committed == {3}
        match = conn.execute(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'third' AND generation_id=3"
        ).fetchone()[0]
        assert match >= 1, "FTS MATCH must still work after prune rebuild"
    finally:
        conn.close()


def test_prune_with_no_stale_generations_is_a_noop(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("only"))
    statements: list[str] = []
    conn = sqlite3.connect(store.path)
    try:
        conn.set_trace_callback(statements.append)
        conn.execute("BEGIN IMMEDIATE")
        CodeIntelStore._prune(conn, keep=2, current=1)
        conn.commit()
        assert not any("DROP TABLE" in stmt.upper() for stmt in statements)
    finally:
        conn.close()


def test_search_survives_repeated_prune_rebuilds(tmp_path: Path) -> None:
    store = CodeIntelStore(tmp_path)
    for index in range(5):
        store.save_graph(_graph(f"handler_{index}"))
    hits = store.search("handler_4")
    assert hits and hits[0]["name"] == "handler_4"
    # Older-generation rows are gone from FTS but the retained previous
    # generation is still searchable through load_graph.
    assert store.load_graph(4) is not None


def test_fk_child_columns_are_indexed(tmp_path: Path) -> None:
    """Payload compaction deletes ~N parent rows per build; without child-side
    indexes SQLite enforces each delete with a full membership-table scan
    (quadratic — 533s of a 536s save at 50k nodes, hours at repo scale)."""
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("one"))
    with sqlite3.connect(store.path) as conn:
        indexes = {
            row[0]
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='index'"
            )
        }
    for required in (
        "idx_generation_nodes_payload",
        "idx_generation_edges_payload",
        "idx_generation_dead_payload",
        "idx_generation_analysis_payload",
    ):
        assert required in indexes, f"missing FK child index {required}"


# ---------------------------------------------------------------------------
# 2. Worker self-supervision: deadline trip → interrupt → escalate.
# ---------------------------------------------------------------------------


def test_self_supervisor_trips_interrupts_then_escalates() -> None:
    from devcouncil.codeintel.build_worker import SelfSupervisor

    events: list[str] = []
    supervisor = SelfSupervisor(
        total_timeout=0.2,
        interrupt=lambda: events.append("interrupt"),
        escalate=lambda: events.append("escalate"),
        escalate_after=0.4,
        poll_interval=0.05,
        on_trip=lambda reason: events.append(f"trip:{reason}"),
    )
    supervisor.start()
    try:
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline and "escalate" not in events:
            time.sleep(0.05)
    finally:
        supervisor.stop()
    assert any(event.startswith("trip:") for event in events), events
    assert "interrupt" in events, "supervisor must interrupt the active statement"
    assert "escalate" in events, "supervisor must escalate when interrupts do not stop the build"
    assert supervisor.tripped_reason


def test_self_supervisor_never_trips_before_deadline() -> None:
    from devcouncil.codeintel.build_worker import SelfSupervisor

    events: list[str] = []
    supervisor = SelfSupervisor(
        total_timeout=60.0,
        interrupt=lambda: events.append("interrupt"),
        escalate=lambda: events.append("escalate"),
        poll_interval=0.05,
    )
    supervisor.start()
    time.sleep(0.3)
    supervisor.stop()
    assert not events
    assert supervisor.tripped_reason == ""


def test_worker_command_carries_total_timeout(tmp_path: Path) -> None:
    from devcouncil.codeintel.build_control import _worker_command

    command = _worker_command(
        tmp_path,
        build_id="abc123",
        heartbeat_interval=5.0,
        liveness=True,
        changed_file=None,
        total_timeout=1234.5,
    )
    assert "--total-timeout" in command
    assert "1234.5" in command
    assert "--no-liveness" not in command


def test_store_interrupt_writes_aborts_running_statement(tmp_path: Path) -> None:
    import threading

    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("victim"))

    started = threading.Event()
    errors: list[BaseException] = []

    def long_write() -> None:
        try:
            with store._connect() as conn:
                started.set()
                conn.execute(
                    """WITH RECURSIVE spin(x) AS (
                           SELECT 1 UNION ALL SELECT x + 1 FROM spin WHERE x < 300000000
                       )
                       INSERT INTO metadata(key, value)
                       SELECT 'spin', COUNT(*) FROM spin"""
                )
        except BaseException as exc:  # noqa: BLE001 - captured for assertion
            errors.append(exc)

    thread = threading.Thread(target=long_write, daemon=True)
    thread.start()
    assert started.wait(5.0)
    time.sleep(0.2)
    for _ in range(50):
        if store.interrupt_writes():
            break
        time.sleep(0.1)
    thread.join(timeout=10.0)
    assert not thread.is_alive(), "interrupt_writes must abort the running write statement"
    assert errors and isinstance(errors[0], sqlite3.OperationalError)


# ---------------------------------------------------------------------------
# 3. Supervisor kill path must reap SIGTERM-ignoring stragglers.
# ---------------------------------------------------------------------------


@pytest.mark.skipif(os.name == "nt", reason="POSIX process groups")
def test_terminate_worker_reaps_sigterm_ignoring_stragglers() -> None:
    from devcouncil.codeintel.build_control import _terminate_worker

    leader_source = (
        "import os, signal, subprocess, sys, time\n"
        "child = subprocess.Popen([sys.executable, '-c', "
        "'import signal, time, sys; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        "print(\"ready\", flush=True); time.sleep(120)'], stdout=subprocess.PIPE, text=True)\n"
        "child.stdout.readline()\n"  # straggler has installed SIG_IGN before we report it
        "print(child.pid, flush=True)\n"
        "time.sleep(120)\n"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", leader_source],
        stdout=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    assert process.stdout is not None
    straggler_pid = int(process.stdout.readline())
    try:
        still_alive = _terminate_worker(process)
        assert not still_alive
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            try:
                os.kill(straggler_pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.1)
        else:
            pytest.fail(
                "SIGTERM-ignoring straggler survived _terminate_worker; group "
                "SIGKILL must always follow leader death"
            )
    finally:
        for pid in (process.pid, straggler_pid):
            try:
                os.killpg(pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError, OSError):
                pass
            try:
                os.kill(pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError, OSError):
                pass
        process.wait(timeout=5.0)


# ---------------------------------------------------------------------------
# 4. Build-artifact garbage collection.
# ---------------------------------------------------------------------------


def test_stale_build_artifacts_are_garbage_collected(tmp_path: Path) -> None:
    from devcouncil.codeintel.build_control import gc_build_artifacts

    codeintel = tmp_path / ".devcouncil" / "codeintel"
    codeintel.mkdir(parents=True)
    old_time = time.time() - 3 * 86400

    old_changed = codeintel / "changed-deadbeef.txt"
    old_changed.write_text("a.py\n", encoding="utf-8")
    os.utime(old_changed, (old_time, old_time))

    old_tmp = codeintel / ".build_status.json.abc123.tmp"
    old_tmp.write_text("{}", encoding="utf-8")
    os.utime(old_tmp, (old_time, old_time))

    fresh_changed = codeintel / "changed-cafef00d.txt"
    fresh_changed.write_text("b.py\n", encoding="utf-8")

    removed = gc_build_artifacts(tmp_path)

    assert not old_changed.exists()
    assert not old_tmp.exists()
    assert fresh_changed.exists()
    assert removed >= 2


def test_isolated_build_gc_uses_config_derived_handoff_age(tmp_path: Path, monkeypatch) -> None:
    """The per-build GC must reap handoffs on the worker-deadline scale, not days.

    A SIGKILLed hook supervisor (and its worker) skips both ``finally`` unlinks,
    so ``changed-*.txt`` cleanup falls to the GC at the start of the next build.
    The worker self-terminates at ``total_timeout + 30s``; a handoff older than
    twice that provably belongs to a dead build and must not wait out the 2-day
    default.
    """
    from types import SimpleNamespace

    from devcouncil.codeintel import build_control

    (tmp_path / ".devcouncil").mkdir()
    total_timeout = 120.0
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda _r: SimpleNamespace(
            indexing=SimpleNamespace(
                build_stall_timeout_seconds=1.0,
                build_total_timeout_seconds=total_timeout,
            )
        ),
    )
    service = SimpleNamespace(store=SimpleNamespace(current_generation=lambda: 0))
    monkeypatch.setattr(build_control, "get_codeintel_service", lambda _r: service)

    seen: dict[str, float] = {}

    def _capture_gc(root: Path, *, max_age_seconds: float = -1.0) -> int:
        seen["max_age_seconds"] = max_age_seconds
        return 0

    monkeypatch.setattr(build_control, "gc_build_artifacts", _capture_gc)

    def _no_spawn(*_a, **_k):
        raise OSError("spawn blocked by test")

    monkeypatch.setattr(build_control.subprocess, "Popen", _no_spawn)

    with pytest.raises(OSError, match="spawn blocked"):
        build_control.run_isolated_full_build(tmp_path)

    assert seen["max_age_seconds"] == pytest.approx(2.0 * (total_timeout + 30.0))


# ---------------------------------------------------------------------------
# 5. Index freshness must be computed and surfaced, and `dev map dead`
#    must fail loud on a stale index.
# ---------------------------------------------------------------------------


def test_index_freshness_reports_stale_and_fresh_heads(tmp_path: Path) -> None:
    from devcouncil.codeintel.service import index_freshness

    head = _init_repo_with_commit(tmp_path)
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("one", head="0000000000000000000000000000000000000000"))

    stale = index_freshness(tmp_path)
    assert stale["fresh"] is False
    assert stale["current_head"] == head
    assert stale["index_head"] == "0000000000000000000000000000000000000000"
    assert stale["generation"] == 1

    store.save_graph(_graph("two", head=head))
    fresh = index_freshness(tmp_path)
    assert fresh["fresh"] is True
    assert fresh["index_head"] == head


def test_index_freshness_unknown_without_git(tmp_path: Path) -> None:
    from devcouncil.codeintel.service import index_freshness

    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("one"))
    result = index_freshness(tmp_path)
    assert result["fresh"] is None
    assert result["reason"]


def test_graph_dead_fails_loud_on_stale_index(tmp_path: Path) -> None:
    from devcouncil.cli.commands.graph_cmd import app

    _init_repo_with_commit(tmp_path)
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("one", head="0000000000000000000000000000000000000000"))

    runner = CliRunner()
    result = runner.invoke(app, ["dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 3, (
        "a dead-code report from an index built at another commit must not "
        f"exit 0 (got {result.exit_code}); output: {result.output}"
    )
    assert "stale" in result.output.lower()

    allowed = runner.invoke(
        app, ["dead", "--project-root", str(tmp_path), "--allow-stale"]
    )
    assert allowed.exit_code == 0, allowed.output

    as_json = runner.invoke(
        app, ["dead", "--project-root", str(tmp_path), "--json", "--allow-stale"]
    )
    assert as_json.exit_code == 0, as_json.output
    import json as json_module

    # CliRunner merges the stderr staleness banner with stdout; parse the body.
    body = as_json.output[as_json.output.index("{") : as_json.output.rindex("}") + 1]
    payload = json_module.loads(body)
    assert payload["index_freshness"]["fresh"] is False


def test_graph_dead_exits_zero_when_index_matches_head(tmp_path: Path) -> None:
    from devcouncil.cli.commands.graph_cmd import app

    head = _init_repo_with_commit(tmp_path)
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("one", head=head))

    runner = CliRunner()
    result = runner.invoke(app, ["dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
