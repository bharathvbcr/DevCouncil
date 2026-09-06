"""Hardening regressions for the dev map build pipeline.

Each test here encodes a defect observed in production on 2026-08-11:
- ``_prune`` deleting FTS5 rows via the UNINDEXED ``generation_id`` column ran a
  single statement for 2+ hours at 100% CPU (full vtab scan + inverted-index
  churn inside one ever-growing WAL transaction).
- ``dev map dead`` reported dead-code results from an index frozen at a commit
  five days behind HEAD with no staleness signal at all.

The supervised-worker regressions that used to live here (self-enforced
deadline, group SIGKILL for SIGTERM-ignoring stragglers, ``changed-*.txt``
handoff GC) went with ``build_worker`` / ``run_isolated_full_build``: the Rust
kernel builds in its own process and supervises itself.
"""

from __future__ import annotations

import sqlite3
import subprocess
import time
from pathlib import Path

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
# 2. A long write must be abortable.
# ---------------------------------------------------------------------------


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
# 3. Index freshness must be computed and surfaced, and `dev map dead`
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

    payload = json_module.loads(as_json.stdout)
    assert payload["index_freshness"]["fresh"] is False


def test_graph_dead_exits_zero_when_index_matches_head(tmp_path: Path) -> None:
    from devcouncil.cli.commands.graph_cmd import app

    head = _init_repo_with_commit(tmp_path)
    store = CodeIntelStore(tmp_path)
    store.save_graph(_graph("one", head=head))

    runner = CliRunner()
    result = runner.invoke(app, ["dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
