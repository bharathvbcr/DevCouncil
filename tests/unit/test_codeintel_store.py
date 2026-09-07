"""What is left of the Python code-intelligence store: a root, and runtime evidence.

This file held 24 tests of ``CodeIntelStore``: committed generations, atomic
pruning, content-addressed payload reuse, FTS search, an extraction cache,
rename aliases, unresolved-reference recording, the compatibility-export
handshake, corruption quarantine and the v1→v2 migration. Every one of them
exercised code with no production caller: the Rust kernel became the only writer
of the graph, ``write_code_graph`` and ``load_code_graph`` were the store's last
two callers, and both had lost theirs by Lane M3. A test of a function nothing
calls is not coverage, so they went with the store.

The two that were testing something still reachable are here, retargeted:
``canonical_project_root`` (whose caller is
``integrations/mcp/handlers/codeintel.py``) and the runtime-observation gate
(whose writer is the opt-in debug tracer and whose reader is ``run_cypher``).
"""

from __future__ import annotations

from pathlib import Path

from devcouncil.codeintel.service import canonical_project_root, get_codeintel_service
from devcouncil.codeintel.store import RuntimeEvidenceStore


def test_service_canonicalizes_nested_project_paths(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    nested = tmp_path / "src" / "pkg"
    nested.mkdir(parents=True)

    assert canonical_project_root(nested) == tmp_path.resolve()
    assert get_codeintel_service(nested) is get_codeintel_service(tmp_path)


def test_runtime_observation_gate_is_false_until_something_is_observed(
    tmp_path: Path,
) -> None:
    """``has_runtime_observations`` gates a git-shelling fingerprint, so it must
    answer False without creating the store, and True only once a session has
    actually recorded an edge."""
    store = RuntimeEvidenceStore(tmp_path)
    assert store.has_runtime_observations() is False
    assert store.exists() is False, "a read created the store"

    session = store.start_runtime_session(
        provider="pytest", source_fingerprint="fp", build_fingerprint="bp"
    )
    assert store.has_runtime_observations() is False, "an empty session is not evidence"

    store.add_runtime_observations(session, [{"source": "a", "target": "b"}])
    assert store.has_runtime_observations() is True


def test_runtime_store_reuses_an_existing_index_sqlite(tmp_path: Path) -> None:
    """A checkout upgraded from an older DevCouncil still has an ``index.sqlite``
    holding the retired graph tables at ``user_version = 2``. Opening it must
    neither refuse it for its version nor rewrite it: the two runtime tables are
    created ``IF NOT EXISTS`` and everything else is left alone."""
    import sqlite3

    path = tmp_path / ".devcouncil" / "codeintel" / "index.sqlite"
    path.parent.mkdir(parents=True)
    legacy = sqlite3.connect(path)
    legacy.executescript(
        "CREATE TABLE generations (id INTEGER PRIMARY KEY);"
        "INSERT INTO generations VALUES (7);"
        "PRAGMA user_version=2;"
    )
    legacy.commit()
    legacy.close()

    store = RuntimeEvidenceStore(tmp_path)
    session = store.start_runtime_session(
        provider="pytest", source_fingerprint="fp", build_fingerprint="bp"
    )
    store.add_runtime_observations(session, [{"source": "a", "target": "b"}])
    assert store.has_runtime_observations() is True

    conn = sqlite3.connect(path)
    try:
        assert conn.execute("SELECT id FROM generations").fetchall() == [(7,)]
        assert int(conn.execute("PRAGMA user_version").fetchone()[0]) == 2
    finally:
        conn.close()
