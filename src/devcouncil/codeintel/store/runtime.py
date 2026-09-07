"""SQLite store for runtime evidence: debugger sessions and observed edges.

What this file replaces
=======================

``store/sqlite.py`` held ``CodeIntelStore``: 1,832 lines of versioned graph
store — generations, content-addressed node/edge/dead payloads, a file
inventory with contents, analysis shards, rename aliases, an FTS index, an
extraction cache and a compatibility-export handshake. It was the Python
engine's canonical store. The Rust kernel took over extraction, resolution and
persistence, and by Lane M3 its last two production callers,
``load_code_graph`` and ``write_code_graph``, had none of their own. Everything
that store held was then written by nothing and read by nothing.

Two tables were the exception, and they are what is left here. The opt-in debug
tracer (``dev debug``, the ``devcouncil_debug_*`` MCP tools) still records what
a running program actually called, scoped to the fingerprint of the source it
ran against; ``run_cypher`` still merges those edges into the kernel's graph.
That is a real feature with a live writer and a live reader, so it keeps a
store — a small one that says what it is.

Schema
======

The two tables are created ``IF NOT EXISTS`` and this store never reads or
writes ``PRAGMA user_version``. A checkout upgraded from an older DevCouncil
still has an ``index.sqlite`` at ``user_version = 2`` holding both these tables
*and* the retired graph tables; opening it must neither refuse it (a version
ladder that has lost its rungs) nor rewrite it. The graph tables are left
exactly where they are: they are the user's data, they are no longer touched by
anything, and silently dropping tables on open is not this module's decision to
make. ``dev map doctor`` no longer reports on this file at all.
"""

from __future__ import annotations

import json
import logging
import sqlite3
import threading
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator, Sequence

logger = logging.getLogger(__name__)

#: Where the runtime store lives, relative to the project root. Kept at the
#: name the debug tracer has always written so an existing session's evidence
#: is still found after this module replaced the one beside it.
INDEX_REL = Path(".devcouncil") / "codeintel" / "index.sqlite"

_SCHEMA = """
CREATE TABLE IF NOT EXISTS runtime_sessions (
    id TEXT PRIMARY KEY,
    created_at REAL NOT NULL,
    ended_at REAL,
    provider TEXT NOT NULL,
    source_fingerprint TEXT NOT NULL,
    build_fingerprint TEXT NOT NULL,
    executable_hash TEXT NOT NULL DEFAULT '',
    metadata TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS runtime_observations (
    session_id TEXT NOT NULL REFERENCES runtime_sessions(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    source TEXT NOT NULL,
    target TEXT NOT NULL,
    kind TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 1,
    first_seen REAL NOT NULL,
    last_seen REAL NOT NULL,
    evidence TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (session_id, ordinal)
);
"""


def _json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


class RuntimeEvidenceStore:
    """Runtime observations for one resolved project root."""

    def __init__(self, project_root: Path, *, path: Path | None = None):
        self.project_root = project_root.expanduser().resolve()
        self.path = (path or (self.project_root / INDEX_REL)).expanduser().resolve()
        self._init_lock = threading.Lock()

    def exists(self) -> bool:
        return self.path.is_file()

    def initialize(self) -> None:
        with self._init_lock:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with self._connect() as conn:
                conn.executescript(_SCHEMA)
                conn.commit()

    @contextmanager
    def _connect(self, *, readonly: bool = False) -> Iterator[sqlite3.Connection]:
        if readonly:
            uri = f"file:{self.path.as_posix()}?mode=ro"
            conn = sqlite3.connect(uri, uri=True, timeout=5.0)
        else:
            conn = sqlite3.connect(self.path, timeout=30.0)
        try:
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA foreign_keys=ON")
            conn.execute("PRAGMA busy_timeout=5000")
            if not readonly:
                conn.execute("PRAGMA journal_mode=WAL")
                conn.execute("PRAGMA synchronous=NORMAL")
            yield conn
        finally:
            # Close even when pragma setup raises (e.g. a corrupt file): a
            # leaked handle keeps the file open, which blocks a rename on
            # Windows.
            conn.close()

    def start_runtime_session(
        self,
        *,
        provider: str,
        source_fingerprint: str,
        build_fingerprint: str,
        executable_hash: str = "",
        metadata: dict[str, Any] | None = None,
        session_id: str | None = None,
    ) -> str:
        self.initialize()
        session_id = session_id or uuid.uuid4().hex
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO runtime_sessions(
                    id, created_at, provider, source_fingerprint, build_fingerprint,
                    executable_hash, metadata
                ) VALUES(?, ?, ?, ?, ?, ?, ?)""",
                (
                    session_id,
                    time.time(),
                    provider,
                    source_fingerprint,
                    build_fingerprint,
                    executable_hash,
                    _json(metadata or {}),
                ),
            )
            conn.commit()
        return session_id

    def add_runtime_observations(
        self,
        session_id: str,
        observations: Sequence[dict[str, Any]],
    ) -> int:
        if not observations:
            return 0
        now = time.time()
        valid = [row for row in observations if row.get("source") and row.get("target")]
        with self._connect() as conn:
            start = int(conn.execute(
                "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM runtime_observations WHERE session_id=?",
                (session_id,),
            ).fetchone()[0])
            conn.executemany(
                """INSERT INTO runtime_observations(
                    session_id, ordinal, source, target, kind, count,
                    first_seen, last_seen, evidence
                ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                [
                    (
                        session_id,
                        start + index,
                        str(row.get("source", "")),
                        str(row.get("target", "")),
                        str(row.get("kind", "observed_calls")),
                        max(1, int(row.get("count", 1))),
                        float(row.get("first_seen", now)),
                        float(row.get("last_seen", now)),
                        _json(row.get("evidence") or {}),
                    )
                    for index, row in enumerate(valid)
                ],
            )
            conn.commit()
        return len(valid)

    def end_runtime_session(self, session_id: str) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE runtime_sessions SET ended_at=? WHERE id=?",
                (time.time(), session_id),
            )
            conn.commit()

    def has_runtime_observations(self) -> bool:
        if not self.exists():
            return False
        try:
            with self._connect(readonly=True) as conn:
                row = conn.execute(
                    "SELECT EXISTS(SELECT 1 FROM runtime_observations)"
                ).fetchone()
        except sqlite3.DatabaseError:
            # A file that exists but has no runtime tables (or is damaged) holds
            # no observations. Reported as "none", never as an exception out of
            # a merge that is additive by design.
            logger.debug("runtime store unreadable", exc_info=True)
            return False
        return bool(row[0])

    def runtime_observations(
        self,
        *,
        source_fingerprint: str | None = None,
        build_fingerprint: str | None = None,
        executable_hash: str | None = None,
        include_stale: bool = False,
        limit: int = 10_000,
    ) -> list[dict[str, Any]]:
        if not self.exists():
            return []
        filters: list[str] = []
        params: list[Any] = []
        expected = (
            ("s.source_fingerprint", source_fingerprint),
            ("s.build_fingerprint", build_fingerprint),
            ("s.executable_hash", executable_hash),
        )
        if not include_stale:
            for column, value in expected:
                if value is not None:
                    filters.append(f"{column}=?")
                    params.append(value)
        where = f"WHERE {' AND '.join(filters)}" if filters else ""
        params.append(max(1, min(100_000, limit)))
        with self._connect(readonly=True) as conn:
            rows = conn.execute(
                f"""SELECT o.*, s.provider, s.source_fingerprint, s.build_fingerprint,
                            s.executable_hash, s.created_at AS session_created_at
                     FROM runtime_observations o
                     JOIN runtime_sessions s ON s.id=o.session_id
                     {where}
                     ORDER BY o.last_seen DESC LIMIT ?""",
                params,
            ).fetchall()
        return [
            {
                **dict(row),
                "evidence": json.loads(row["evidence"]),
                "fingerprint_matches": all(
                    value is None or row[column.removeprefix("s.")] == value
                    for column, value in expected
                ),
            }
            for row in rows
        ]
