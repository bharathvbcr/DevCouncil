"""Versioned SQLite graph store with atomic committed generations."""

from __future__ import annotations

import hashlib
import json
import logging
import sqlite3
import threading
import time
import uuid
import zlib
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterator, Sequence, TypeVar

from devcouncil.indexing.graph.schema import (
    CodeGraph,
    Confidence,
    DeadCodeEntry,
    GraphEdge,
    GraphNode,
    NodeKind,
)

STORE_SCHEMA_VERSION = 2
ANALYZER_VERSION = "codeintel-1"
INDEX_REL = Path(".devcouncil") / "codeintel" / "index.sqlite"
EXTRACTION_CACHE_MAX_BYTES = 64 * 1024 * 1024
AMBIGUOUS_EVIDENCE_LIMIT = 8

logger = logging.getLogger(__name__)

# Error texts that mean the database file itself is damaged. Lock/busy
# conditions (OperationalError) are transient and must never quarantine.
_CORRUPTION_MARKERS = ("malformed", "not a database", "database disk image")

# Row-at-a-time INSERTs across a few hundred thousand nodes/edges dominate build
# wall clock and emit no progress. Persist runs in batches of this size so the
# supervisor sees a heartbeat per batch. Overridable per-project via
# ``indexing.store_write_batch_size``.
DEFAULT_WRITE_BATCH = 2_000

# Above this many changed paths, ``path NOT IN (?,?,…)`` needs one bind
# parameter per path — past SQLITE_MAX_VARIABLE_NUMBER and pathological to plan.
# Larger sets go through a temporary table instead.
_CHANGED_PATH_TEMP_TABLE_THRESHOLD = 200

# Single definition shared by migration and the prune-time rebuild so the two
# can never drift. ``generation_id`` stays UNINDEXED — FTS5 cannot index it —
# which is exactly why rows must never be deleted through it (see ``_prune``).
_NODES_FTS_DDL = """CREATE VIRTUAL TABLE nodes_fts USING fts5(
                    generation_id UNINDEXED,
                    node_id UNINDEXED,
                    name,
                    qualified_name,
                    path,
                    tokenize='unicode61'
                )"""

_T = TypeVar("_T")


def _batched(items: Sequence[_T], size: int) -> Iterator[tuple[int, Sequence[_T]]]:
    """Yield ``(offset, chunk)`` pairs so callers can report progress."""
    size = max(1, int(size))
    for start in range(0, len(items), size):
        yield start, items[start : start + size]


def _corruption_error(exc: sqlite3.DatabaseError) -> bool:
    text = str(exc).lower()
    if any(marker in text for marker in _CORRUPTION_MARKERS):
        return True
    if isinstance(exc, sqlite3.OperationalError):
        return False
    return False


@dataclass(frozen=True)
class StoreStatus:
    project_root: str
    database: str
    schema_version: int
    generation: int | None
    state: str
    node_count: int = 0
    edge_count: int = 0
    created_at: float | None = None
    analyzer_version: str = ANALYZER_VERSION

    def as_dict(self) -> dict[str, Any]:
        return {
            "project_root": self.project_root,
            "database": self.database,
            "schema_version": self.schema_version,
            "generation": self.generation,
            "state": self.state,
            "node_count": self.node_count,
            "edge_count": self.edge_count,
            "created_at": self.created_at,
            "analyzer_version": self.analyzer_version,
        }


def _json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), default=str)


def compatibility_graph_digest(graph: CodeGraph) -> str:
    """Digest the public graph payload before store-only normalization."""
    payload = graph.model_dump(mode="json")
    return hashlib.sha256(_json(payload).encode("utf-8")).hexdigest()


def _confidence_score(edge: GraphEdge) -> float:
    raw = edge.extras.get("confidence_score")
    if isinstance(raw, (int, float)):
        return max(0.0, min(1.0, float(raw)))
    confidence = edge.confidence.value if hasattr(edge.confidence, "value") else str(edge.confidence)
    return {"extracted": 1.0, "inferred": 0.7, "ambiguous": 0.4}.get(confidence, 0.4)


def _provenance(edge: GraphEdge) -> str:
    raw = edge.extras.get("provenance")
    if isinstance(raw, str) and raw in {"extracted", "framework", "inferred", "runtime", "user"}:
        return raw
    confidence = edge.confidence.value if hasattr(edge.confidence, "value") else str(edge.confidence)
    return "extracted" if confidence == "extracted" else "inferred"


def _canonicalize_duplicate_ids(graph: CodeGraph) -> CodeGraph:
    """Give repeated legacy symbol ids explicit structural identities.

    Graph v2 historically identified a symbol by ``path::qualname``.  That is
    ambiguous for repeated declarations (including test redefinitions and
    generated properties), while the transactional store requires one stable
    row per identity.  Preserve the first compatibility id and attach a
    line/kind discriminator plus an alias edge to later declarations.
    """
    grouped: dict[str, list[GraphNode]] = {}
    for node in graph.nodes:
        grouped.setdefault(node.id, []).append(node)
    duplicates = {node_id: nodes for node_id, nodes in grouped.items() if len(nodes) > 1}
    if not duplicates:
        return graph

    normalized = graph.model_copy(deep=True)
    normalized_grouped: dict[str, list[GraphNode]] = {}
    for node in normalized.nodes:
        normalized_grouped.setdefault(node.id, []).append(node)

    aliases: list[dict[str, object]] = []
    assigned: set[str] = {node.id for node in normalized.nodes}
    replacement_by_location: dict[tuple[str, int, str], str] = {}
    for old_id, nodes in normalized_grouped.items():
        if len(nodes) < 2:
            continue
        for ordinal, node in enumerate(nodes[1:], start=2):
            kind = node.kind.value if hasattr(node.kind, "value") else str(node.kind)
            discriminator = f"L{node.line}:{kind}"
            candidate = f"{old_id}#{discriminator}"
            suffix = ordinal
            while candidate in assigned:
                candidate = f"{old_id}#{discriminator}:{suffix}"
                suffix += 1
            assigned.add(candidate)
            node.id = candidate
            node.extras["identity_alias"] = old_id
            node.extras["structural_discriminator"] = discriminator
            replacement_by_location[(old_id, node.line, kind)] = candidate
            aliases.append(
                {
                    "old_id": old_id,
                    "new_id": candidate,
                    "reason": "duplicate legacy symbol identity",
                }
            )
            normalized.edges.append(
                GraphEdge(
                    source=node.path,
                    target=candidate,
                    kind="contains",
                    confidence=Confidence.EXTRACTED,
                    reason="structurally disambiguated definition",
                    extras={"provenance": "extracted", "identity_alias": old_id},
                )
            )
            normalized.edges.append(
                GraphEdge(
                    source=candidate,
                    target=old_id,
                    kind="aliases",
                    confidence=Confidence.EXTRACTED,
                    reason="duplicate legacy symbol identity",
                    extras={"provenance": "user", "structural_discriminator": discriminator},
                )
            )

    # Repeated definitions produced repeated identical containment rows.  Keep
    # the compatibility edge once; the disambiguated definitions have their
    # own structural containment edge above.
    seen_structural: set[tuple[str, str, str, str]] = set()
    edges: list[GraphEdge] = []
    for edge in normalized.edges:
        key = (edge.source, edge.target, edge.kind, edge.reason)
        if edge.kind == "contains" and edge.target in duplicates:
            if key in seen_structural:
                continue
            seen_structural.add(key)
        edges.append(edge)
    normalized.edges = edges

    for entry in normalized.dead_code:
        replacement = replacement_by_location.get((entry.id, entry.line, entry.kind))
        if replacement is not None:
            entry.id = replacement
    normalized.meta["duplicate_symbol_aliases"] = aliases
    return normalized


class CodeIntelStore:
    """Canonical graph store for one resolved project root.

    A writer creates all rows for a new generation in one transaction and only
    then advances ``current_generation``. Readers therefore see the complete old
    or complete new graph, never a mixed refresh.
    """

    def __init__(self, project_root: Path, *, path: Path | None = None):
        self.project_root = project_root.expanduser().resolve()
        self.path = (path or (self.project_root / INDEX_REL)).expanduser().resolve()
        self._init_lock = threading.Lock()
        self.last_write_stats: dict[str, int] = {}
        self._active_write_conn: sqlite3.Connection | None = None
        self._active_write_lock = threading.Lock()

    def interrupt_writes(self) -> bool:
        """Abort the statement running on the active write connection, if any.

        ``sqlite3.Connection.interrupt()`` is safe to call from another thread;
        the interrupted statement raises ``OperationalError: interrupted`` and
        the surrounding transaction rolls back. Returns True when a write
        connection was open to interrupt. This is the only way a watchdog can
        stop a wedged multi-hour statement — signals cannot reach a thread
        blocked inside SQLite C code.
        """
        with self._active_write_lock:
            conn = self._active_write_conn
            if conn is None:
                return False
            try:
                conn.interrupt()
            except sqlite3.Error:
                return False
            return True

    def exists(self) -> bool:
        return self.path.is_file()

    def quarantine_if_corrupt(self, exc: sqlite3.DatabaseError) -> bool:
        """Move a damaged store aside so the next write rebuilds from scratch.

        Returns True when the error signals file corruption and the store was
        quarantined (``index.sqlite`` → ``index.sqlite.corrupt``). Lock/busy
        errors and schema-level errors return False and must be raised by the
        caller as before.
        """
        if not _corruption_error(exc):
            return False
        if not self.quarantine():
            return False
        logger.warning(
            "codeintel store is corrupt (%s); quarantined to %s — rebuilding from scratch",
            exc,
            self.path.name + ".corrupt",
        )
        return True

    def quarantine(self) -> bool:
        """Move the store file (and WAL/SHM siblings) aside to ``*.corrupt``."""
        quarantine = self.path.with_name(self.path.name + ".corrupt")
        try:
            for suffix in ("", "-wal", "-shm"):
                source = Path(str(self.path) + suffix)
                if source.exists():
                    target = Path(str(quarantine) + suffix)
                    target.unlink(missing_ok=True)
                    source.replace(target)
        except OSError:
            logger.warning("failed to quarantine corrupt codeintel store", exc_info=True)
            return False
        return True

    def initialize(self) -> None:
        with self._init_lock:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with self._connect() as conn:
                self._migrate(conn)

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
                conn.execute("PRAGMA auto_vacuum=INCREMENTAL")
                # The default ~2MB page cache thrashes on gigabyte-scale
                # indexes: every btree descent during a generation commit
                # becomes a disk read plus a WAL-frame probe (observed as
                # hours of pread/walFindFrame during persist:commit). 64MB
                # keeps the hot interior pages resident for the transaction.
                conn.execute("PRAGMA cache_size=-65536")
                with self._active_write_lock:
                    self._active_write_conn = conn
            yield conn
        finally:
            if not readonly:
                with self._active_write_lock:
                    if self._active_write_conn is conn:
                        self._active_write_conn = None
            # Close even when pragma setup raises (e.g. a corrupt file): a
            # leaked handle keeps the file open, which blocks the Windows
            # rename in quarantine().
            conn.close()

    def _migrate(self, conn: sqlite3.Connection) -> None:
        version = int(conn.execute("PRAGMA user_version").fetchone()[0])
        if version > STORE_SCHEMA_VERSION:
            raise RuntimeError(
                f"Code-intelligence database schema {version} is newer than supported "
                f"schema {STORE_SCHEMA_VERSION}. Upgrade DevCouncil."
            )
        if version == 0:
            conn.executescript(
                """
                CREATE TABLE metadata (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE generations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    state TEXT NOT NULL CHECK(state IN ('building', 'committed', 'failed')),
                    created_at REAL NOT NULL,
                    analyzer_version TEXT NOT NULL,
                    schema_version INTEGER NOT NULL,
                    generated_head TEXT NOT NULL DEFAULT '',
                    indexed_hash TEXT NOT NULL DEFAULT '',
                    content_fingerprint TEXT NOT NULL DEFAULT '',
                    graph_meta TEXT NOT NULL DEFAULT '{}',
                    node_count INTEGER NOT NULL DEFAULT 0,
                    edge_count INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE files (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    path TEXT NOT NULL,
                    language TEXT NOT NULL DEFAULT '',
                    content_hash TEXT NOT NULL DEFAULT '',
                    size INTEGER NOT NULL DEFAULT 0,
                    mtime_ns INTEGER NOT NULL DEFAULT 0,
                    content BLOB,
                    PRIMARY KEY (generation_id, path)
                );
                CREATE TABLE nodes (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    path TEXT NOT NULL DEFAULT '',
                    name TEXT NOT NULL DEFAULT '',
                    line INTEGER NOT NULL DEFAULT 0,
                    end_line INTEGER NOT NULL DEFAULT 0,
                    area TEXT NOT NULL DEFAULT '',
                    language TEXT NOT NULL DEFAULT '',
                    exported INTEGER NOT NULL DEFAULT 0,
                    community TEXT NOT NULL DEFAULT '',
                    extras TEXT NOT NULL DEFAULT '{}',
                    PRIMARY KEY (generation_id, id)
                );
                CREATE TABLE edges (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    target TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    confidence TEXT NOT NULL,
                    confidence_score REAL NOT NULL,
                    provenance TEXT NOT NULL,
                    reason TEXT NOT NULL DEFAULT '',
                    evidence TEXT NOT NULL DEFAULT '[]',
                    analyzer_version TEXT NOT NULL,
                    source_fingerprint TEXT NOT NULL DEFAULT '',
                    extras TEXT NOT NULL DEFAULT '{}',
                    PRIMARY KEY (generation_id, ordinal)
                );
                CREATE TABLE dead_code (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL,
                    id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    line INTEGER NOT NULL DEFAULT 0,
                    kind TEXT NOT NULL DEFAULT '',
                    confidence TEXT NOT NULL,
                    reason TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (generation_id, ordinal)
                );
                CREATE TABLE unresolved_references (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    source_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    path TEXT NOT NULL DEFAULT '',
                    line INTEGER NOT NULL DEFAULT 0,
                    evidence TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE diagnostics (
                    generation_id INTEGER REFERENCES generations(id) ON DELETE CASCADE,
                    path TEXT NOT NULL DEFAULT '',
                    severity TEXT NOT NULL,
                    code TEXT NOT NULL,
                    message TEXT NOT NULL,
                    data TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE aliases (
                    old_id TEXT NOT NULL,
                    new_id TEXT NOT NULL,
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    reason TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (old_id, generation_id)
                );
                CREATE TABLE extraction_cache (
                    content_hash TEXT NOT NULL,
                    language TEXT NOT NULL,
                    grammar_version TEXT NOT NULL,
                    analyzer_version TEXT NOT NULL,
                    config_hash TEXT NOT NULL,
                    payload BLOB NOT NULL,
                    created_at REAL NOT NULL,
                    PRIMARY KEY (content_hash, language, grammar_version, analyzer_version, config_hash)
                );
                CREATE TABLE runtime_sessions (
                    id TEXT PRIMARY KEY,
                    created_at REAL NOT NULL,
                    ended_at REAL,
                    provider TEXT NOT NULL,
                    source_fingerprint TEXT NOT NULL,
                    build_fingerprint TEXT NOT NULL,
                    executable_hash TEXT NOT NULL DEFAULT '',
                    metadata TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE runtime_observations (
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
                + _NODES_FTS_DDL
                + """;
                CREATE INDEX idx_nodes_path ON nodes(generation_id, path);
                CREATE INDEX idx_nodes_name ON nodes(generation_id, name);
                CREATE INDEX idx_edges_source ON edges(generation_id, source, kind);
                CREATE INDEX idx_edges_target ON edges(generation_id, target, kind);
                CREATE INDEX idx_unresolved_name ON unresolved_references(generation_id, name);
                PRAGMA user_version=1;
                """
            )
            conn.commit()
            version = 1
        if version == 1:
            conn.executescript(
                """
                CREATE TABLE node_payloads (
                    payload_hash TEXT PRIMARY KEY,
                    payload BLOB NOT NULL
                );
                CREATE TABLE generation_nodes (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL,
                    payload_hash TEXT NOT NULL REFERENCES node_payloads(payload_hash),
                    path TEXT NOT NULL DEFAULT '',
                    node_id TEXT NOT NULL,
                    PRIMARY KEY (generation_id, node_id)
                );
                CREATE TABLE edge_payloads (
                    payload_hash TEXT PRIMARY KEY,
                    payload BLOB NOT NULL
                );
                CREATE TABLE generation_edges (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL,
                    payload_hash TEXT NOT NULL REFERENCES edge_payloads(payload_hash),
                    source_path TEXT NOT NULL DEFAULT '',
                    target_path TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (generation_id, ordinal, payload_hash)
                );
                CREATE TABLE dead_payloads (
                    payload_hash TEXT PRIMARY KEY,
                    payload BLOB NOT NULL
                );
                CREATE TABLE generation_dead (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL,
                    payload_hash TEXT NOT NULL REFERENCES dead_payloads(payload_hash),
                    path TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (generation_id, ordinal, payload_hash)
                );
                CREATE TABLE file_contents (
                    content_hash TEXT PRIMARY KEY,
                    content BLOB
                );
                CREATE TABLE generation_files (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    path TEXT NOT NULL,
                    language TEXT NOT NULL DEFAULT '',
                    content_hash TEXT NOT NULL DEFAULT '',
                    size INTEGER NOT NULL DEFAULT 0,
                    mtime_ns INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (generation_id, path)
                );
                CREATE TABLE analysis_payloads (
                    payload_hash TEXT PRIMARY KEY,
                    payload BLOB NOT NULL
                );
                CREATE TABLE generation_analysis (
                    generation_id INTEGER NOT NULL REFERENCES generations(id) ON DELETE CASCADE,
                    path TEXT NOT NULL,
                    payload_hash TEXT NOT NULL REFERENCES analysis_payloads(payload_hash),
                    PRIMARY KEY (generation_id, path)
                );
                CREATE INDEX idx_generation_nodes_path
                    ON generation_nodes(generation_id, path);
                CREATE INDEX idx_generation_edges_source
                    ON generation_edges(generation_id, source_path);
                CREATE INDEX idx_generation_edges_target
                    ON generation_edges(generation_id, target_path);
                PRAGMA user_version=2;
                """
            )
            self._migrate_v1_payloads(conn)
            conn.commit()
        # Idempotent ensure-step, deliberately outside the versioned ladder so
        # older readers stay compatible: FK child columns must be indexed.
        # Payload compaction deletes tens of thousands of parent rows per
        # build, and SQLite enforces each parent delete by scanning the child
        # table when the referencing column has no index — a quadratic that
        # ran persist:commit for hours at repo scale (533s of a 536s save at
        # 50k nodes; verified by phase profile on 2026-08-11).
        conn.executescript(
            """
            CREATE INDEX IF NOT EXISTS idx_generation_nodes_payload
                ON generation_nodes(payload_hash);
            CREATE INDEX IF NOT EXISTS idx_generation_edges_payload
                ON generation_edges(payload_hash);
            CREATE INDEX IF NOT EXISTS idx_generation_dead_payload
                ON generation_dead(payload_hash);
            CREATE INDEX IF NOT EXISTS idx_generation_analysis_payload
                ON generation_analysis(payload_hash);
            """
        )
        conn.commit()

    def _migrate_v1_payloads(self, conn: sqlite3.Connection) -> None:
        """Copy retained v1 generations into content-addressed payload tables."""
        for row in conn.execute("SELECT * FROM nodes ORDER BY generation_id, rowid"):
            payload = {
                key: row[key]
                for key in (
                    "id", "kind", "path", "name", "line", "end_line", "area",
                    "language", "exported", "community", "extras",
                )
            }
            payload["exported"] = bool(payload["exported"])
            payload["extras"] = json.loads(payload["extras"])
            digest, blob = self._payload(payload)
            conn.execute("INSERT OR IGNORE INTO node_payloads VALUES(?, ?)", (digest, blob))
            ordinal = int(conn.execute(
                "SELECT COUNT(*) FROM generation_nodes WHERE generation_id=?",
                (row["generation_id"],),
            ).fetchone()[0])
            conn.execute(
                "INSERT OR IGNORE INTO generation_nodes VALUES(?, ?, ?, ?, ?)",
                (row["generation_id"], ordinal, digest, row["path"], row["id"]),
            )
        for row in conn.execute("SELECT * FROM edges ORDER BY generation_id, ordinal"):
            payload = {
                key: row[key]
                for key in (
                    "source", "target", "kind", "confidence", "confidence_score",
                    "provenance", "reason", "evidence", "analyzer_version",
                    "source_fingerprint", "extras",
                )
            }
            payload["extras"] = json.loads(payload["extras"])
            digest, blob = self._payload(payload)
            conn.execute("INSERT OR IGNORE INTO edge_payloads VALUES(?, ?)", (digest, blob))
            conn.execute(
                "INSERT OR IGNORE INTO generation_edges VALUES(?, ?, ?, ?, ?)",
                (
                    row["generation_id"], row["ordinal"], digest,
                    self._identity_path(row["source"]), self._identity_path(row["target"]),
                ),
            )
        for row in conn.execute("SELECT * FROM dead_code ORDER BY generation_id, ordinal"):
            payload = {
                key: row[key]
                for key in ("id", "path", "line", "kind", "confidence", "reason")
            }
            digest, blob = self._payload(payload)
            conn.execute("INSERT OR IGNORE INTO dead_payloads VALUES(?, ?)", (digest, blob))
            conn.execute(
                "INSERT OR IGNORE INTO generation_dead VALUES(?, ?, ?, ?)",
                (row["generation_id"], row["ordinal"], digest, row["path"]),
            )
        for row in conn.execute("SELECT * FROM files ORDER BY generation_id, path"):
            if row["content_hash"]:
                conn.execute(
                    "INSERT OR IGNORE INTO file_contents VALUES(?, ?)",
                    (row["content_hash"], row["content"]),
                )
            conn.execute(
                "INSERT OR IGNORE INTO generation_files VALUES(?, ?, ?, ?, ?, ?)",
                (
                    row["generation_id"], row["path"], row["language"],
                    row["content_hash"], row["size"], row["mtime_ns"],
                ),
            )
        # The v2 payload/membership tables are canonical. Keep legacy tables
        # present for rollback-compatible schema inspection, but not duplicated.
        conn.execute("DELETE FROM diagnostics")
        conn.execute("DELETE FROM dead_code")
        conn.execute("DELETE FROM edges")
        conn.execute("DELETE FROM nodes")
        conn.execute("DELETE FROM files")

    @staticmethod
    def _payload(value: dict[str, Any]) -> tuple[str, bytes]:
        raw = _json(value).encode("utf-8")
        return hashlib.sha256(raw).hexdigest(), zlib.compress(raw, 6)

    @staticmethod
    def _decode_payload(blob: bytes) -> dict[str, Any]:
        return dict(json.loads(zlib.decompress(blob)))

    @staticmethod
    def _identity_path(identity: str) -> str:
        return identity.split("::", 1)[0].replace("\\", "/")

    def current_generation(self) -> int | None:
        if not self.exists():
            return None
        with self._connect(readonly=True) as conn:
            row = conn.execute("SELECT value FROM metadata WHERE key='current_generation'").fetchone()
        return int(row[0]) if row is not None else None

    def generation_provenance(self) -> dict[str, Any] | None:
        """Provenance of the committed generation readers currently see.

        Freshness checks compare ``generated_head`` against the working tree's
        HEAD so a frozen index can never masquerade as current evidence.
        """
        if not self.exists():
            return None
        with self._connect(readonly=True) as conn:
            row = conn.execute("SELECT value FROM metadata WHERE key='current_generation'").fetchone()
            if row is None:
                return None
            generation = int(row[0])
            record = conn.execute(
                """SELECT generated_head, created_at, analyzer_version
                     FROM generations WHERE id=? AND state='committed'""",
                (generation,),
            ).fetchone()
        if record is None:
            return None
        return {
            "generation": generation,
            "generated_head": str(record["generated_head"] or ""),
            "created_at": float(record["created_at"]),
            "analyzer_version": str(record["analyzer_version"] or ""),
        }

    def save_graph(
        self,
        graph: CodeGraph,
        *,
        retain_generations: int = 2,
        changed_paths: set[str] | None = None,
        analysis_shards: dict[str, dict[str, Any]] | None = None,
        progress: Callable[[str, int, int], None] | None = None,
    ) -> int:
        self.initialize()
        batch = self._write_batch_size()

        def report(phase: str, completed: int, total: int) -> None:
            if progress is None:
                return
            try:
                progress(phase, completed, total)
            except Exception:  # noqa: BLE001 - progress must never fail a build
                logger.debug("persist progress callback failed", exc_info=True)

        compatibility_digest = compatibility_graph_digest(graph)
        graph = _canonicalize_duplicate_ids(graph)
        graph_meta = dict(graph.meta)
        graph_meta.pop("communities", None)
        meta = {
            "dead_code": len(graph.dead_code),
            "entry_roots": graph.entry_roots,
            "unwired_candidates": graph.unwired_candidates,
            "unreachable_files": graph.unreachable_files,
            "meta": graph_meta,
        }
        with self._connect() as conn:
            try:
                conn.execute("BEGIN IMMEDIATE")
                previous_row = conn.execute(
                    "SELECT value FROM metadata WHERE key='current_generation'"
                ).fetchone()
                previous_generation = int(previous_row[0]) if previous_row is not None else None
                cursor = conn.execute(
                    """INSERT INTO generations(
                        state, created_at, analyzer_version, schema_version,
                        generated_head, indexed_hash, content_fingerprint, graph_meta
                    ) VALUES('building', ?, ?, ?, ?, ?, ?, ?)""",
                    (
                        time.time(),
                        ANALYZER_VERSION,
                        graph.schema_version,
                        graph.generated_head,
                        graph.indexed_hash,
                        graph.content_fingerprint,
                        _json(meta),
                    ),
                )
                if cursor.lastrowid is None:
                    raise RuntimeError("SQLite did not return a generation id")
                generation = int(cursor.lastrowid)
                normalized_changed = {
                    path.replace("\\", "/") for path in (changed_paths or set())
                }
                # Empty changed_paths must not enter incremental mode: _copy_unaffected
                # returns immediately when the exclusion set is empty, which would
                # commit a generation with zero memberships.
                incremental = (
                    previous_generation is not None
                    and changed_paths is not None
                    and bool(normalized_changed)
                )
                if incremental:
                    assert previous_generation is not None
                    self._copy_unaffected_memberships(
                        conn, previous_generation, generation, normalized_changed
                    )
                file_nodes: Sequence[GraphNode] = (
                    [node for node in graph.nodes if node.path in normalized_changed]
                    if incremental else graph.nodes
                )
                report("persist:files", 0, len(file_nodes))
                file_rows = self._file_rows(generation, file_nodes)
                for offset, chunk in _batched(file_rows, batch):
                    contents = [
                        (row[3], row[6])
                        for row in chunk
                        if row[3] and row[6] is not None
                    ]
                    if contents:
                        conn.executemany(
                            "INSERT OR IGNORE INTO file_contents(content_hash, content) "
                            "VALUES(?, ?)",
                            contents,
                        )
                    conn.executemany(
                        """INSERT OR REPLACE INTO generation_files(
                            generation_id, path, language, content_hash, size, mtime_ns
                        ) VALUES(?, ?, ?, ?, ?, ?)""",
                        [row[:6] for row in chunk],
                    )
                    report("persist:files", offset + len(chunk), len(file_rows))
                node_payload_writes = 0
                edge_payload_writes = 0
                dead_payload_writes = 0

                node_rows = [
                    (index, node)
                    for index, node in enumerate(graph.nodes)
                    if not incremental or node.path in normalized_changed
                ]
                report("persist:nodes", 0, len(node_rows))
                for offset, chunk in _batched(node_rows, batch):
                    payloads: list[tuple[Any, ...]] = []
                    memberships: list[tuple[Any, ...]] = []
                    for index, node in chunk:
                        digest, blob = self._payload(self._node_payload(node))
                        payloads.append((digest, blob))
                        memberships.append((generation, index, digest, node.path, node.id))
                    node_payload_writes += conn.executemany(
                        "INSERT OR IGNORE INTO node_payloads VALUES(?, ?)", payloads
                    ).rowcount
                    conn.executemany(
                        "INSERT OR REPLACE INTO generation_nodes VALUES(?, ?, ?, ?, ?)",
                        memberships,
                    )
                    report("persist:nodes", offset + len(chunk), len(node_rows))

                edge_rows = []
                for index, edge in enumerate(graph.edges):
                    source_path = self._identity_path(edge.source)
                    target_path = self._identity_path(edge.target)
                    if incremental and not ({source_path, target_path} & normalized_changed):
                        continue
                    edge_rows.append((index, edge, source_path, target_path))
                report("persist:edges", 0, len(edge_rows))
                for offset, chunk in _batched(edge_rows, batch):
                    payloads = []
                    memberships = []
                    for index, edge, source_path, target_path in chunk:
                        digest, blob = self._payload(self._edge_payload(edge))
                        payloads.append((digest, blob))
                        memberships.append(
                            (generation, index, digest, source_path, target_path)
                        )
                    edge_payload_writes += conn.executemany(
                        "INSERT OR IGNORE INTO edge_payloads VALUES(?, ?)", payloads
                    ).rowcount
                    conn.executemany(
                        "INSERT OR IGNORE INTO generation_edges VALUES(?, ?, ?, ?, ?)",
                        memberships,
                    )
                    report("persist:edges", offset + len(chunk), len(edge_rows))

                unresolved = self._unresolved_rows(generation, graph)
                conn.executemany(
                    """INSERT INTO unresolved_references(
                        generation_id, source_id, name, kind, path, line, evidence
                    ) VALUES(?, ?, ?, ?, ?, ?, ?)""",
                    unresolved,
                )
                dead_rows = [
                    (index, entry)
                    for index, entry in enumerate(graph.dead_code)
                    if not incremental or entry.path in normalized_changed
                ]
                report("persist:dead", 0, len(dead_rows))
                for offset, chunk in _batched(dead_rows, batch):
                    payloads = []
                    memberships = []
                    for index, entry in chunk:
                        digest, blob = self._payload(self._dead_payload(entry))
                        payloads.append((digest, blob))
                        memberships.append((generation, index, digest, entry.path))
                    dead_payload_writes += conn.executemany(
                        "INSERT OR IGNORE INTO dead_payloads VALUES(?, ?)", payloads
                    ).rowcount
                    conn.executemany(
                        "INSERT OR IGNORE INTO generation_dead VALUES(?, ?, ?, ?)",
                        memberships,
                    )
                    report("persist:dead", offset + len(chunk), len(dead_rows))
                report("persist:fts", 0, 1)
                conn.executemany(
                    "INSERT INTO nodes_fts(generation_id, node_id, name, qualified_name, path) VALUES(?, ?, ?, ?, ?)",
                    [
                        (generation, node.id, node.name, node.id.rsplit("::", 1)[-1], node.path)
                        for node in graph.nodes
                        if not incremental or node.path in normalized_changed
                    ],
                )
                report("persist:fts", 1, 1)
                if analysis_shards is not None:
                    report("persist:shards", 0, len(analysis_shards))
                    self._write_analysis_shards(
                        conn, generation, analysis_shards, normalized_changed if incremental else None
                    )
                    report("persist:shards", len(analysis_shards), len(analysis_shards))
                if previous_generation is not None:
                    self._record_rename_aliases(conn, previous_generation, generation)
                report("persist:commit", 0, 1)
                conn.execute(
                    "UPDATE generations SET state='committed', node_count=?, edge_count=? WHERE id=?",
                    (len(graph.nodes), len(graph.edges), generation),
                )
                conn.execute(
                    "INSERT INTO metadata(key, value) VALUES('current_generation', ?) "
                    "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    (str(generation),),
                )
                conn.execute(
                    "INSERT INTO metadata(key, value) VALUES('compatibility_export_digest', ?) "
                    "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    (compatibility_digest,),
                )
                self._prune(conn, keep=max(1, retain_generations), current=generation)
                self._compact_payloads(conn)
                self.last_write_stats = {
                    "node_payloads_written": node_payload_writes,
                    "edge_payloads_written": edge_payload_writes,
                    "dead_payloads_written": dead_payload_writes,
                    "node_memberships": int(conn.execute(
                        "SELECT COUNT(*) FROM generation_nodes WHERE generation_id=?",
                        (generation,),
                    ).fetchone()[0]),
                    "edge_memberships": int(conn.execute(
                        "SELECT COUNT(*) FROM generation_edges WHERE generation_id=?",
                        (generation,),
                    ).fetchone()[0]),
                }
                conn.commit()
                report("persist:commit", 1, 1)
                # Truncate the WAL now that the generation is durable. Without
                # this a long build leaves a WAL that keeps growing across runs
                # (hundreds of MB), and every later read pays walFindFrame on it.
                self._checkpoint(conn)
                return generation
            except Exception:
                conn.rollback()
                raise

    def _write_batch_size(self) -> int:
        """Rows per persist batch (``indexing.store_write_batch_size``)."""
        try:
            from devcouncil.app.config import load_config

            return max(1, int(load_config(self.project_root).indexing.store_write_batch_size))
        except Exception:
            return DEFAULT_WRITE_BATCH

    def _store_file_contents(self) -> bool:
        """Whether to persist compressed file bytes (``indexing.store_file_contents``)."""
        try:
            from devcouncil.app.config import load_config

            return bool(load_config(self.project_root).indexing.store_file_contents)
        except Exception:
            return False

    @staticmethod
    def _checkpoint(conn: sqlite3.Connection) -> None:
        """Best-effort WAL truncate after a committed generation."""
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        except sqlite3.Error:
            # A concurrent reader can hold the WAL open; PASSIVE still reclaims
            # what it can and a later write retries.
            try:
                conn.execute("PRAGMA wal_checkpoint(PASSIVE)")
            except sqlite3.Error:
                logger.debug("wal checkpoint failed", exc_info=True)

    def compatibility_export_state(self) -> tuple[str, int | None]:
        """Return the last public graph digest and observed export mtime."""
        if not self.exists():
            return "", None
        with self._connect(readonly=True) as conn:
            rows = conn.execute(
                "SELECT key, value FROM metadata WHERE key IN "
                "('compatibility_export_digest', 'compatibility_export_mtime_ns')"
            ).fetchall()
        values = {str(row["key"]): str(row["value"]) for row in rows}
        raw_mtime = values.get("compatibility_export_mtime_ns")
        return values.get("compatibility_export_digest", ""), (
            int(raw_mtime) if raw_mtime is not None else None
        )

    def record_compatibility_export(self, path: Path, graph: CodeGraph) -> None:
        """Mark an on-disk JSON artifact as the export for this generation."""
        self.initialize()
        mtime_ns = path.stat().st_mtime_ns
        digest = compatibility_graph_digest(graph)
        with self._connect() as conn:
            conn.executemany(
                "INSERT INTO metadata(key, value) VALUES(?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [
                    ("compatibility_export_digest", digest),
                    ("compatibility_export_mtime_ns", str(mtime_ns)),
                ],
            )
            conn.commit()

    def _file_rows(self, generation: int, nodes: Sequence[GraphNode]) -> list[tuple[Any, ...]]:
        by_path: dict[str, str] = {}
        for node in nodes:
            if node.path:
                by_path.setdefault(node.path, node.language)
        rows: list[tuple[Any, ...]] = []
        store_contents = self._store_file_contents()
        for rel, language in sorted(by_path.items()):
            path = self.project_root / rel
            try:
                raw = path.read_bytes()
                stat = path.stat()
            except OSError:
                rows.append((generation, rel, language, "", 0, 0, None))
                continue
            digest = hashlib.sha256(raw).hexdigest()
            # Content blobs are what make index.sqlite grow to gigabytes on a
            # large repo. Hash/size/mtime are always kept (they drive change
            # detection); the bytes themselves are opt-in and otherwise read
            # back from the working tree by ``content_for_path``.
            blob = zlib.compress(raw, 6) if store_contents else None
            rows.append(
                (generation, rel, language, digest, len(raw), stat.st_mtime_ns, blob)
            )
        return rows

    @staticmethod
    def _node_payload(node: GraphNode) -> dict[str, Any]:
        return node.model_dump(mode="json")

    @staticmethod
    def _edge_payload(edge: GraphEdge) -> dict[str, Any]:
        payload = edge.model_dump(mode="json")
        evidence = payload["extras"].get("evidence")
        if (
            payload["confidence"] == Confidence.AMBIGUOUS.value
            and isinstance(evidence, list)
            and len(evidence) > AMBIGUOUS_EVIDENCE_LIMIT
        ):
            payload["extras"]["evidence"] = evidence[:AMBIGUOUS_EVIDENCE_LIMIT]
            payload["extras"]["evidence_truncated"] = len(evidence) - AMBIGUOUS_EVIDENCE_LIMIT
        return payload

    @staticmethod
    def _dead_payload(entry: DeadCodeEntry) -> dict[str, Any]:
        return entry.model_dump(mode="json")

    @staticmethod
    def _copy_unaffected_memberships(
        conn: sqlite3.Connection,
        previous: int,
        generation: int,
        changed: set[str],
    ) -> None:
        if not changed:
            return
        # Inlining one bind parameter per changed path breaks past
        # SQLITE_MAX_VARIABLE_NUMBER on a repo-scale change set and forces a
        # bad plan well before that. Beyond the threshold, stage the exclusion
        # set in a temp table and let SQLite use its primary-key index.
        if len(changed) > _CHANGED_PATH_TEMP_TABLE_THRESHOLD:
            conn.execute(
                "CREATE TEMP TABLE IF NOT EXISTS changed_paths (path TEXT PRIMARY KEY)"
            )
            conn.execute("DELETE FROM changed_paths")
            conn.executemany(
                "INSERT OR IGNORE INTO changed_paths(path) VALUES(?)",
                [(path,) for path in sorted(changed)],
            )
            exclusion = "SELECT path FROM changed_paths"
            params: tuple[Any, ...] = (generation, previous)
            edge_params: tuple[Any, ...] = params
        else:
            exclusion = ",".join("?" for _ in changed)
            params = (generation, previous, *sorted(changed))
            edge_params = (generation, previous, *sorted(changed), *sorted(changed))
        try:
            conn.execute(
                f"""INSERT INTO generation_files
                    SELECT ?, path, language, content_hash, size, mtime_ns
                      FROM generation_files
                     WHERE generation_id=? AND path NOT IN ({exclusion})""",  # noqa: S608
                params,
            )
            conn.execute(
                f"""INSERT INTO generation_nodes
                    SELECT ?, ordinal, payload_hash, path, node_id
                      FROM generation_nodes
                     WHERE generation_id=? AND path NOT IN ({exclusion})""",  # noqa: S608
                params,
            )
            conn.execute(
                f"""INSERT INTO generation_edges
                    SELECT ?, ordinal, payload_hash, source_path, target_path
                      FROM generation_edges
                     WHERE generation_id=?
                       AND source_path NOT IN ({exclusion})
                       AND target_path NOT IN ({exclusion})""",  # noqa: S608
                edge_params,
            )
            conn.execute(
                f"""INSERT INTO generation_dead
                    SELECT ?, ordinal, payload_hash, path
                      FROM generation_dead
                     WHERE generation_id=? AND path NOT IN ({exclusion})""",  # noqa: S608
                params,
            )
            conn.execute(
                f"""INSERT INTO generation_analysis
                    SELECT ?, path, payload_hash
                      FROM generation_analysis
                     WHERE generation_id=? AND path NOT IN ({exclusion})""",  # noqa: S608
                params,
            )
            conn.execute(
                f"""INSERT INTO nodes_fts(generation_id, node_id, name, qualified_name, path)
                    SELECT ?, node_id, name, qualified_name, path
                      FROM nodes_fts
                     WHERE generation_id=? AND path NOT IN ({exclusion})""",  # noqa: S608
                params,
            )
        finally:
            if len(changed) > _CHANGED_PATH_TEMP_TABLE_THRESHOLD:
                try:
                    conn.execute("DELETE FROM changed_paths")
                except sqlite3.Error:
                    logger.debug("could not clear changed_paths temp table", exc_info=True)

    def _write_analysis_shards(
        self,
        conn: sqlite3.Connection,
        generation: int,
        shards: dict[str, dict[str, Any]],
        changed: set[str] | None,
    ) -> None:
        for path, shard in shards.items():
            normalized = path.replace("\\", "/")
            if changed is not None and normalized not in changed:
                continue
            digest, blob = self._payload(shard)
            conn.execute("INSERT OR IGNORE INTO analysis_payloads VALUES(?, ?)", (digest, blob))
            conn.execute(
                "INSERT OR REPLACE INTO generation_analysis VALUES(?, ?, ?)",
                (generation, normalized, digest),
            )

    @staticmethod
    def _compact_payloads(conn: sqlite3.Connection) -> None:
        for payload_table, membership_table in (
            ("node_payloads", "generation_nodes"),
            ("edge_payloads", "generation_edges"),
            ("dead_payloads", "generation_dead"),
            ("analysis_payloads", "generation_analysis"),
        ):
            conn.execute(
                f"""DELETE FROM {payload_table}
                     WHERE payload_hash NOT IN (
                         SELECT DISTINCT payload_hash FROM {membership_table}
                     )"""  # noqa: S608
            )
        conn.execute(
            """DELETE FROM file_contents
                 WHERE content_hash NOT IN (
                     SELECT DISTINCT content_hash FROM generation_files
                     WHERE content_hash<>''
                 )"""
        )
        # Pruning a generation frees tens of thousands of pages; reclaiming 64
        # per build (256KB) lets a gigabyte-scale freelist accumulate forever.
        # 8192 pages (~32MB) keeps reclaim bounded but meaningful.
        conn.execute("PRAGMA incremental_vacuum(8192)")

    @staticmethod
    def _node_row(generation: int, node: GraphNode) -> tuple[Any, ...]:
        kind = node.kind.value if hasattr(node.kind, "value") else str(node.kind)
        return (
            generation,
            node.id,
            kind,
            node.path,
            node.name,
            node.line,
            node.end_line,
            node.area,
            node.language,
            int(node.exported),
            node.community,
            _json(node.extras),
        )

    @staticmethod
    def _edge_row(generation: int, ordinal: int, edge: GraphEdge) -> tuple[Any, ...]:
        confidence = edge.confidence.value if hasattr(edge.confidence, "value") else str(edge.confidence)
        evidence = edge.extras.get("evidence", [])
        fingerprint = str(edge.extras.get("source_fingerprint", ""))
        return (
            generation,
            ordinal,
            edge.source,
            edge.target,
            edge.kind,
            confidence,
            _confidence_score(edge),
            _provenance(edge),
            edge.reason,
            _json(evidence),
            ANALYZER_VERSION,
            fingerprint,
            _json(edge.extras),
        )

    @staticmethod
    def _unresolved_rows(generation: int, graph: CodeGraph) -> list[tuple[Any, ...]]:
        incoming: dict[str, str] = {}
        for edge in graph.edges:
            if edge.kind == "dynamic_reference":
                incoming.setdefault(edge.target, edge.source)
        rows: list[tuple[Any, ...]] = []
        for node in graph.nodes:
            kind = node.kind.value if hasattr(node.kind, "value") else str(node.kind)
            if kind != "dynamic" or node.extras.get("resolved") is not False:
                continue
            rows.append((
                generation,
                incoming.get(node.id, node.path),
                node.name,
                str(node.extras.get("sink") or "dynamic"),
                node.path,
                node.line,
                _json({"node_id": node.id, "extras": node.extras}),
            ))
        for raw in graph.meta.get("unresolved_references") or []:
            if not isinstance(raw, dict) or not raw.get("name"):
                continue
            rows.append((
                generation,
                str(raw.get("source_id") or raw.get("path") or ""),
                str(raw["name"]),
                str(raw.get("kind") or "reference"),
                str(raw.get("path") or ""),
                int(raw.get("line") or 0),
                _json(raw.get("evidence") or {}),
            ))
        return rows

    def _record_rename_aliases(
        self, conn: sqlite3.Connection, previous: int, current: int
    ) -> None:
        """Preserve identities across same-content file renames as explicit aliases.

        A rename is a path that left the index whose content reappeared at
        exactly one new path. Joining generations on content alone would
        cross-link every pair of identical files (empty ``__init__.py``,
        generated boilerplate) into false aliases with quadratic node lookups.
        """
        removed = conn.execute(
            """SELECT path, content_hash FROM generation_files
                WHERE generation_id=? AND content_hash<>''
                  AND path NOT IN (
                      SELECT path FROM generation_files WHERE generation_id=?
                  )""",
            (previous, current),
        ).fetchall()
        added = conn.execute(
            """SELECT path, content_hash FROM generation_files
                WHERE generation_id=? AND content_hash<>''
                  AND path NOT IN (
                      SELECT path FROM generation_files WHERE generation_id=?
                  )""",
            (current, previous),
        ).fetchall()
        removed_by_hash: dict[str, list[str]] = {}
        for row in removed:
            removed_by_hash.setdefault(str(row["content_hash"]), []).append(str(row["path"]))
        added_by_hash: dict[str, list[str]] = {}
        for row in added:
            added_by_hash.setdefault(str(row["content_hash"]), []).append(str(row["path"]))
        renamed = [
            (old_paths[0], new_paths[0])
            for content_hash, old_paths in sorted(removed_by_hash.items())
            if len(old_paths) == 1
            and len(new_paths := added_by_hash.get(content_hash, [])) == 1
        ]
        for old_path, new_path in renamed:
            old_nodes = self._nodes_for_path(conn, previous, old_path)
            new_nodes = self._nodes_for_path(conn, current, new_path)
            by_shape = {
                (
                    node.kind.value, node.name if node.kind != NodeKind.FILE else "",
                    node.line, node.end_line,
                ): node
                for node in new_nodes
            }
            for old in old_nodes:
                key = (
                    old.kind.value, old.name if old.kind != NodeKind.FILE else "",
                    old.line, old.end_line,
                )
                new = by_shape.get(key)
                if new is not None:
                    conn.execute(
                        "INSERT OR IGNORE INTO aliases VALUES(?, ?, ?, ?)",
                        (old.id, new.id, current, "same-content file rename"),
                    )

    def _nodes_for_path(
        self, conn: sqlite3.Connection, generation: int, path: str
    ) -> list[GraphNode]:
        return [
            GraphNode.model_validate(self._decode_payload(row["payload"]))
            for row in conn.execute(
                """SELECT p.payload FROM generation_nodes m
                   JOIN node_payloads p ON p.payload_hash=m.payload_hash
                   WHERE m.generation_id=? AND m.path=?""",
                (generation, path),
            )
        ]

    @staticmethod
    def _dead_row(generation: int, ordinal: int, entry: DeadCodeEntry) -> tuple[Any, ...]:
        confidence = entry.confidence.value if hasattr(entry.confidence, "value") else str(entry.confidence)
        return (generation, ordinal, entry.id, entry.path, entry.line, entry.kind, confidence, entry.reason)

    @staticmethod
    def _prune(conn: sqlite3.Connection, *, keep: int, current: int) -> None:
        rows = conn.execute(
            "SELECT id FROM generations WHERE state='committed' ORDER BY id DESC"
        ).fetchall()
        stale = [int(row[0]) for row in rows[keep:] if int(row[0]) != current]
        if not stale:
            return
        # ``DELETE FROM nodes_fts WHERE generation_id=?`` cannot use an index
        # (FTS5 UNINDEXED column): it full-scans the virtual table and pays
        # inverted-index churn for every deleted row, with each lookup walking
        # the transaction's own ever-growing WAL. On a repo-scale generation
        # that one statement runs for hours at 100% CPU. Rebuilding from the
        # kept rows is linear and bounded: copy kept rows out, drop the shadow
        # trees wholesale, re-insert.
        kept = [int(row[0]) for row in rows if int(row[0]) not in set(stale)]
        placeholders = ",".join("?" for _ in kept)
        conn.execute("DROP TABLE IF EXISTS temp.nodes_fts_keep")
        if kept:
            conn.execute(
                f"""CREATE TEMP TABLE nodes_fts_keep AS
                    SELECT generation_id, node_id, name, qualified_name, path
                      FROM nodes_fts WHERE generation_id IN ({placeholders})""",  # noqa: S608
                kept,
            )
        conn.execute("DROP TABLE nodes_fts")
        conn.execute(_NODES_FTS_DDL)
        if kept:
            conn.execute(
                """INSERT INTO nodes_fts(generation_id, node_id, name, qualified_name, path)
                   SELECT generation_id, node_id, name, qualified_name, path
                     FROM nodes_fts_keep"""
            )
            conn.execute("DROP TABLE nodes_fts_keep")
        for generation in stale:
            conn.execute("DELETE FROM generations WHERE id=?", (generation,))

    def load_graph(self, generation: int | None = None) -> CodeGraph | None:
        if not self.exists():
            return None
        with self._connect(readonly=True) as conn:
            if generation is None:
                row = conn.execute("SELECT value FROM metadata WHERE key='current_generation'").fetchone()
                if row is None:
                    return None
                generation = int(row[0])
            gen = conn.execute(
                "SELECT * FROM generations WHERE id=? AND state='committed'", (generation,)
            ).fetchone()
            if gen is None:
                return None
            nodes = [
                GraphNode.model_validate(self._decode_payload(row["payload"]))
                for row in conn.execute(
                    """SELECT p.payload FROM generation_nodes m
                       JOIN node_payloads p ON p.payload_hash=m.payload_hash
                       WHERE m.generation_id=? ORDER BY m.ordinal, m.node_id""",
                    (generation,),
                )
            ]
            edges = [
                GraphEdge.model_validate(self._decode_payload(row["payload"]))
                for row in conn.execute(
                    """SELECT p.payload FROM generation_edges m
                       JOIN edge_payloads p ON p.payload_hash=m.payload_hash
                       WHERE m.generation_id=? ORDER BY m.ordinal, m.payload_hash""",
                    (generation,),
                )
            ]
            dead = [
                DeadCodeEntry.model_validate(self._decode_payload(row["payload"]))
                for row in conn.execute(
                    """SELECT p.payload FROM generation_dead m
                       JOIN dead_payloads p ON p.payload_hash=m.payload_hash
                       WHERE m.generation_id=? ORDER BY m.ordinal, m.payload_hash""",
                    (generation,),
                )
            ]
        graph_meta = json.loads(gen["graph_meta"])
        return CodeGraph(
            schema_version=int(gen["schema_version"]),
            nodes=nodes,
            edges=edges,
            dead_code=dead,
            entry_roots=list(graph_meta.get("entry_roots") or []),
            unwired_candidates=list(graph_meta.get("unwired_candidates") or []),
            unreachable_files=list(graph_meta.get("unreachable_files") or []),
            generated_head=str(gen["generated_head"]),
            indexed_hash=str(gen["indexed_hash"]),
            content_fingerprint=str(gen["content_fingerprint"]),
            meta={
                **dict(graph_meta.get("meta") or {}),
                "codeintel_generation": generation,
                "codeintel_analyzer_version": str(gen["analyzer_version"]),
            },
        )

    @staticmethod
    def _node_from_row(row: sqlite3.Row) -> GraphNode:
        return GraphNode(
            id=row["id"],
            kind=row["kind"],
            path=row["path"],
            name=row["name"],
            line=row["line"],
            end_line=row["end_line"],
            area=row["area"],
            language=row["language"],
            exported=bool(row["exported"]),
            community=row["community"],
            extras=json.loads(row["extras"]),
        )

    @staticmethod
    def _edge_from_row(row: sqlite3.Row) -> GraphEdge:
        extras = json.loads(row["extras"])
        return GraphEdge(
            source=row["source"],
            target=row["target"],
            kind=row["kind"],
            confidence=row["confidence"],
            reason=row["reason"],
            extras=extras,
        )

    @staticmethod
    def _dead_from_row(row: sqlite3.Row) -> DeadCodeEntry:
        return DeadCodeEntry(
            id=row["id"],
            path=row["path"],
            line=row["line"],
            kind=row["kind"],
            confidence=Confidence(row["confidence"]),
            reason=row["reason"],
        )

    def search(self, query: str, *, limit: int = 50) -> list[dict[str, Any]]:
        generation = self.current_generation()
        if generation is None:
            return []
        terms = " ".join(part for part in query.replace("::", " ").split() if part)
        if not terms:
            return []
        with self._connect(readonly=True) as conn:
            try:
                rows = conn.execute(
                    """SELECT node_id, bm25(nodes_fts) AS rank
                       FROM nodes_fts
                       WHERE nodes_fts MATCH ? AND generation_id=?
                       ORDER BY rank LIMIT ?""",
                    (terms, generation, max(1, min(500, limit))),
                ).fetchall()
            except sqlite3.OperationalError:
                rows = []
            ranked = {str(row["node_id"]): float(row["rank"]) for row in rows}
            if ranked:
                placeholders = ",".join("?" for _ in ranked)
                payload_rows = conn.execute(
                    f"""SELECT m.node_id, p.payload FROM generation_nodes m
                        JOIN node_payloads p ON p.payload_hash=m.payload_hash
                        WHERE m.generation_id=? AND m.node_id IN ({placeholders})""",  # noqa: S608
                    (generation, *ranked),
                ).fetchall()
                found = []
                for row in payload_rows:
                    node = self._decode_payload(row["payload"])
                    found.append({
                        key: node[key]
                        for key in ("id", "kind", "path", "name", "line", "end_line", "area", "language")
                    } | {"rank": ranked[str(row["node_id"])]})
                return sorted(found, key=lambda item: item["rank"])
            lowered = query.casefold()
            fallback = []
            for row in conn.execute(
                """SELECT p.payload FROM generation_nodes m
                   JOIN node_payloads p ON p.payload_hash=m.payload_hash
                   WHERE m.generation_id=?""",
                (generation,),
            ):
                node = self._decode_payload(row["payload"])
                if lowered not in f"{node['id']} {node['name']} {node['path']}".casefold():
                    continue
                fallback.append({
                    key: node[key]
                    for key in ("id", "kind", "path", "name", "line", "end_line", "area", "language")
                } | {"rank": 0.0})
                if len(fallback) >= max(1, min(500, limit)):
                    break
            return fallback

    def status(self) -> StoreStatus:
        if not self.exists():
            return StoreStatus(
                project_root=str(self.project_root),
                database=str(self.path),
                schema_version=STORE_SCHEMA_VERSION,
                generation=None,
                state="uninitialized",
            )
        try:
            with self._connect(readonly=True) as conn:
                schema = int(conn.execute("PRAGMA user_version").fetchone()[0])
                row = conn.execute(
                    """SELECT g.* FROM generations g
                       JOIN metadata m ON m.key='current_generation' AND CAST(m.value AS INTEGER)=g.id"""
                ).fetchone()
        except sqlite3.DatabaseError as exc:
            if not _corruption_error(exc):
                raise
            return StoreStatus(
                str(self.project_root), str(self.path), STORE_SCHEMA_VERSION, None, "corrupt"
            )
        if row is None:
            return StoreStatus(str(self.project_root), str(self.path), schema, None, "empty")
        return StoreStatus(
            project_root=str(self.project_root),
            database=str(self.path),
            schema_version=schema,
            generation=int(row["id"]),
            state=str(row["state"]),
            node_count=int(row["node_count"]),
            edge_count=int(row["edge_count"]),
            created_at=float(row["created_at"]),
            analyzer_version=str(row["analyzer_version"]),
        )

    def content_for_path(self, path: str, *, generation: int | None = None) -> bytes | None:
        generation = generation or self.current_generation()
        if generation is None:
            return None
        with self._connect(readonly=True) as conn:
            row = conn.execute(
                """SELECT c.content FROM generation_files f
                   LEFT JOIN file_contents c ON c.content_hash=f.content_hash
                   WHERE f.generation_id=? AND f.path=?""",
                (generation, path.replace("\\", "/")),
            ).fetchone()
        if row is not None and row[0] is not None:
            return zlib.decompress(row[0])
        if row is None:
            return None
        # The generation knows the file but content blobs were not stored
        # (``indexing.store_file_contents`` off, the default). Fall back to the
        # working tree so callers keep working; None when it is gone.
        try:
            return (self.project_root / path.replace("\\", "/")).read_bytes()
        except OSError:
            return None

    def file_metadata(self, *, generation: int | None = None) -> dict[str, tuple[int, int, str]]:
        """Return ``path -> (size, mtime_ns, sha256)`` for reconciliation."""

        generation = generation or self.current_generation()
        if generation is None:
            return {}
        with self._connect(readonly=True) as conn:
            rows = conn.execute(
                "SELECT path, size, mtime_ns, content_hash FROM generation_files WHERE generation_id=?",
                (generation,),
            ).fetchall()
        return {
            str(row["path"]): (int(row["size"]), int(row["mtime_ns"]), str(row["content_hash"]))
            for row in rows
        }

    def has_indexed_path(self, path: str, *, generation: int | None = None) -> bool:
        """Single-row membership check (cheap enough for per-event watcher calls)."""
        generation = generation or self.current_generation()
        if generation is None:
            return False
        with self._connect(readonly=True) as conn:
            row = conn.execute(
                "SELECT 1 FROM generation_files WHERE generation_id=? AND path=? LIMIT 1",
                (generation, path.replace("\\", "/")),
            ).fetchone()
        return row is not None

    def analysis_shards(
        self, *, generation: int | None = None
    ) -> dict[str, dict[str, Any]]:
        generation = generation or self.current_generation()
        if generation is None:
            return {}
        with self._connect(readonly=True) as conn:
            rows = conn.execute(
                """SELECT m.path, p.payload FROM generation_analysis m
                   JOIN analysis_payloads p ON p.payload_hash=m.payload_hash
                   WHERE m.generation_id=?""",
                (generation,),
            ).fetchall()
        return {
            str(row["path"]): self._decode_payload(row["payload"])
            for row in rows
        }

    def unresolved_references(self, *, name: str | None = None) -> list[dict[str, Any]]:
        generation = self.current_generation()
        if generation is None:
            return []
        sql = "SELECT * FROM unresolved_references WHERE generation_id=?"
        params: list[Any] = [generation]
        if name:
            sql += " AND name=?"
            params.append(name)
        sql += " ORDER BY path, line, name"
        with self._connect(readonly=True) as conn:
            rows = conn.execute(sql, params).fetchall()
        return [{**dict(row), "evidence": json.loads(row["evidence"])} for row in rows]

    def diagnostics(self) -> list[dict[str, Any]]:
        """Derive unresolved-reference diagnostics without duplicate storage."""
        return [
            {
                "generation_id": row["generation_id"],
                "path": row["path"],
                "severity": "warning",
                "code": "dynamic_unresolved",
                "message": f"Unresolved dynamic reference: {row['name']}",
                "data": row["evidence"],
            }
            for row in self.unresolved_references()
        ]

    def aliases(self) -> list[dict[str, Any]]:
        generation = self.current_generation()
        if generation is None:
            return []
        with self._connect(readonly=True) as conn:
            rows = conn.execute(
                "SELECT old_id, new_id, reason FROM aliases WHERE generation_id=? ORDER BY old_id",
                (generation,),
            ).fetchall()
        return [dict(row) for row in rows]

    def put_extraction(
        self,
        *,
        content_hash: str,
        language: str,
        grammar_version: str,
        config_hash: str,
        payload: bytes,
    ) -> None:
        self.initialize()
        with self._connect() as conn:
            conn.execute(
                """INSERT INTO extraction_cache(
                    content_hash, language, grammar_version, analyzer_version,
                    config_hash, payload, created_at
                ) VALUES(?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(content_hash, language, grammar_version, analyzer_version, config_hash)
                DO UPDATE SET payload=excluded.payload, created_at=excluded.created_at""",
                (content_hash, language, grammar_version, ANALYZER_VERSION, config_hash, payload, time.time()),
            )
            total = int(conn.execute(
                "SELECT COALESCE(SUM(length(payload)), 0) FROM extraction_cache"
            ).fetchone()[0])
            if total > EXTRACTION_CACHE_MAX_BYTES:
                conn.execute(
                    """DELETE FROM extraction_cache WHERE rowid IN (
                        SELECT rowid FROM extraction_cache ORDER BY created_at
                        LIMIT (
                            SELECT MAX(1, COUNT(*) / 4) FROM extraction_cache
                        )
                    )"""
                )
            conn.commit()

    def get_extraction(
        self,
        *,
        content_hash: str,
        language: str,
        grammar_version: str,
        config_hash: str,
    ) -> bytes | None:
        if not self.exists():
            return None
        with self._connect(readonly=True) as conn:
            row = conn.execute(
                """SELECT payload FROM extraction_cache
                   WHERE content_hash=? AND language=? AND grammar_version=?
                     AND analyzer_version=? AND config_hash=?""",
                (content_hash, language, grammar_version, ANALYZER_VERSION, config_hash),
            ).fetchone()
        return bytes(row[0]) if row is not None else None

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
            conn.execute("UPDATE runtime_sessions SET ended_at=? WHERE id=?", (time.time(), session_id))
            conn.commit()

    def has_runtime_observations(self) -> bool:
        if not self.exists():
            return False
        with self._connect(readonly=True) as conn:
            row = conn.execute(
                "SELECT EXISTS(SELECT 1 FROM runtime_observations)"
            ).fetchone()
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
