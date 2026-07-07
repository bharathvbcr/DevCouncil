import logging
from collections import defaultdict
from contextlib import contextmanager
from datetime import datetime, timezone

from sqlmodel import SQLModel, create_engine, Session
from sqlalchemy import inspect, text
from sqlalchemy.exc import OperationalError
from pathlib import Path
from typing import Optional

from devcouncil.storage.models import SchemaVersionModel

logger = logging.getLogger(__name__)


# v3: per-task indexes on the task/audit tables (task_id, lease status).
# v4: machine-routable gap columns (file, line, suggested_command,
#     acceptance_criterion_id) so the repair contract survives a reload.
# v5: partial unique index enforcing one ACTIVE lease per task (single-writer).
# v6: gap column expected_verification_method (repair loop tells an executor-remediable
#     "incomplete" from a manual/llm one across a reload).
# v7: task columns agent_appended_expected_tests_json / agent_appended_allowed_commands_json
#     (agent-negotiated scope extensions). These are NOT NULL with a scalar default, which
#     the column migration now adds as ``ADD COLUMN ... NOT NULL DEFAULT <literal>`` —
#     previously non-nullable additions were skipped entirely, so every SELECT on an
#     existing database crashed with "no such column".
# v8: optional task.priority column (high/medium/low planner hint).
SCHEMA_VERSION = 8


# Module-level caches keyed by *resolved* paths so distinct project roots stay
# independent (important for test isolation, where every test uses a fresh
# tmp_path). `_db_instances` returns the same Database (and its single engine)
# for a given project root instead of rebuilding the engine + re-running the
# schema check on every get_db() call. `_dedup_done` records db paths whose
# active-lease dedup migration has already run this process, so the one-time
# table scan does not repeat on subsequent opens of the same database.
_db_instances: dict[Path, "Database"] = {}
_dedup_done: set[Path] = set()


def reset_db_cache() -> None:
    """Drop all cached Database instances and per-path guards.

    Disposes pooled engine connections so the underlying SQLite files can be
    safely removed/recreated. Intended for tests (or long-lived processes) that
    rebuild a project's .devcouncil database under a path already opened this
    process; without this a cached engine could point at a stale/deleted file.
    """
    for db in _db_instances.values():
        try:
            db.engine.dispose()
        except Exception as e:
            # A failed dispose can leave the SQLite file locked for later rebuilds.
            logger.debug("Failed to dispose cached engine for %s: %s", db.db_path, e)
    _db_instances.clear()
    _dedup_done.clear()


class Database:
    def __init__(self, db_path: Path):
        self.db_path = db_path
        self.engine = create_engine(f"sqlite:///{db_path}")

    def create_db_and_tables(self):
        self._create_tables()
        self.ensure_schema_version()

    def ensure_schema_version(self):
        self._create_tables()
        with Session(self.engine) as session:
            current = session.get(SchemaVersionModel, "singleton")
            if current is None:
                session.add(SchemaVersionModel(id="singleton", version=SCHEMA_VERSION))
                session.commit()
                return
            if current.version > SCHEMA_VERSION:
                raise RuntimeError(
                    f"Unsupported DevCouncil schema version {current.version}; "
                    f"expected {SCHEMA_VERSION}."
                )
            if current.version < SCHEMA_VERSION:
                self._create_tables()
                current.version = SCHEMA_VERSION
                session.add(current)
                session.commit()
                return

    def _create_tables(self):
        try:
            SQLModel.metadata.create_all(self.engine)
        except OperationalError as exc:
            if "already exists" not in str(exc):
                raise
        self._create_missing_columns()
        self._dedup_active_leases()
        self._create_missing_indexes()

    def _dedup_active_leases(self):
        # The partial unique index (ux_task_leases_active) can only be created if no task
        # already has two ACTIVE leases. Older databases predate the constraint, so
        # collapse any duplicates first — keep the newest active lease per task, mark the
        # rest stale — making index creation succeed and restoring single-writer state.
        #
        # This only needs to run once per database per process: after the first pass the
        # partial unique index (created immediately after, in _create_missing_indexes)
        # prevents any new duplicate active leases, so re-scanning on every open is wasted
        # work. Guard on the resolved path so distinct databases remain independent.
        key = self.db_path.resolve()
        if key in _dedup_done:
            return
        inspector = inspect(self.engine)
        if "task_leases" not in inspector.get_table_names():
            # Table not materialized yet; don't record as done so a later open retries.
            return
        with self.engine.begin() as conn:
            rows = conn.execute(
                text("SELECT id, task_id, created_at FROM task_leases WHERE status = 'active'")
            ).fetchall()
            by_task = defaultdict(list)
            for row in rows:
                by_task[row.task_id].append(((row.created_at or ""), row.id))
            stale_ids = []
            for items in by_task.values():
                if len(items) <= 1:
                    continue
                items.sort()  # ascending by (created_at, id); keep the last = newest
                stale_ids.extend(item[1] for item in items[:-1])
            if stale_ids:
                now = datetime.now(timezone.utc).isoformat()
                for lease_id in stale_ids:
                    conn.execute(
                        text("UPDATE task_leases SET status = 'stale', released_at = :ts WHERE id = :id"),
                        {"ts": now, "id": lease_id},
                    )
        _dedup_done.add(key)

    @staticmethod
    def _scalar_default_literal(column) -> Optional[str]:
        """SQL literal for a column's scalar Python-side default, or None.

        Only simple scalars (str/bool/int/float) are rendered — anything callable,
        context-sensitive, or exotic returns None so the caller can skip the column
        rather than emit wrong DDL."""
        default = getattr(column, "default", None)
        if default is None or getattr(default, "is_callable", False):
            return None
        arg = getattr(default, "arg", None)
        if isinstance(arg, bool):
            return "1" if arg else "0"
        if isinstance(arg, (int, float)):
            return str(arg)
        if isinstance(arg, str):
            escaped = arg.replace("'", "''")
            return f"'{escaped}'"
        return None

    def _create_missing_columns(self):
        # create_all never alters an existing table, so columns added to a model
        # later (e.g. the v4 gap routing columns) are missing on databases created by
        # an older version — and a SELECT of the model would then fail. Add any
        # missing column in place via ADD COLUMN: nullable columns as-is, and
        # NON-nullable columns with a scalar default as
        # ``NOT NULL DEFAULT <literal>`` (the shape SQLite accepts without a table
        # rewrite — existing rows take the default). Previously non-nullable
        # additions were skipped entirely, so a model gaining a required column
        # (v7's agent_appended_*_json) crashed every SELECT on an existing database
        # with "no such column". Non-nullable columns WITHOUT a scalar default are
        # still skipped: existing rows could not satisfy them.
        inspector = inspect(self.engine)
        existing_tables = set(inspector.get_table_names())
        for table in SQLModel.metadata.sorted_tables:
            if table.name not in existing_tables:
                continue
            existing_cols = {col["name"] for col in inspector.get_columns(table.name)}
            for column in table.columns:
                if column.name in existing_cols:
                    continue
                ddl_type = column.type.compile(self.engine.dialect)
                if column.nullable:
                    ddl = f'ALTER TABLE "{table.name}" ADD COLUMN "{column.name}" {ddl_type}'
                else:
                    literal = self._scalar_default_literal(column)
                    if literal is None:
                        continue
                    ddl = (
                        f'ALTER TABLE "{table.name}" ADD COLUMN "{column.name}" '
                        f"{ddl_type} NOT NULL DEFAULT {literal}"
                    )
                try:
                    with self.engine.begin() as conn:
                        conn.execute(text(ddl))
                except OperationalError as exc:
                    if "duplicate column name" not in str(exc).lower():
                        raise

    def _create_missing_indexes(self):
        # create_all skips tables that already exist, so indexes added to the
        # model definitions later never materialize on existing databases.
        for table in SQLModel.metadata.sorted_tables:
            for index in table.indexes:
                try:
                    index.create(self.engine, checkfirst=True)
                except OperationalError as exc:
                    if "already exists" not in str(exc):
                        raise

    @contextmanager
    def get_session(self):
        """Yield a session with automatic commit/rollback/close."""
        session = Session(self.engine)
        try:
            yield session
            session.commit()
        except Exception:
            session.rollback()
            raise
        finally:
            session.close()


def get_db(project_root: Path = Path(".")) -> Optional[Database]:
    dev_dir = project_root / ".devcouncil"
    if not dev_dir.exists():
        # Checked before the cache so a not-yet-initialized (or removed) project
        # never yields a stale cached Database.
        return None

    key = project_root.resolve()
    db_path = dev_dir / "state.sqlite"
    cached = _db_instances.get(key)
    # Only reuse the cached instance while its underlying file still exists. If the
    # .devcouncil dir was wiped and recreated under the same path within one process,
    # the cached engine points at a deleted file — dispose its pool and rebuild fresh.
    if cached is not None:
        if db_path.exists():
            return cached
        try:
            cached.engine.dispose()
        except Exception as e:
            logger.debug("Failed to dispose stale cached engine for %s: %s", db_path, e)

    db = Database(db_path)
    db.ensure_schema_version()
    _db_instances[key] = db
    return db
