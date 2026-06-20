from collections import defaultdict
from contextlib import contextmanager
from datetime import datetime, timezone

from sqlmodel import SQLModel, create_engine, Session
from sqlalchemy import inspect, text
from sqlalchemy.exc import OperationalError
from pathlib import Path
from typing import Optional

from devcouncil.storage.models import SchemaVersionModel


# v3: per-task indexes on the task/audit tables (task_id, lease status).
# v4: machine-routable gap columns (file, line, suggested_command,
#     acceptance_criterion_id) so the repair contract survives a reload.
# v5: partial unique index enforcing one ACTIVE lease per task (single-writer).
SCHEMA_VERSION = 5


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
        inspector = inspect(self.engine)
        if "task_leases" not in inspector.get_table_names():
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

    def _create_missing_columns(self):
        # create_all never alters an existing table, so columns added to a model
        # later (e.g. the v4 gap routing columns) are missing on databases created by
        # an older version — and a SELECT of the model would then fail. Add any
        # missing column as a nullable ADD COLUMN (the only shape SQLite can add in
        # place without a table rewrite); non-nullable additions are intentionally
        # skipped because existing rows could not satisfy them.
        inspector = inspect(self.engine)
        existing_tables = set(inspector.get_table_names())
        for table in SQLModel.metadata.sorted_tables:
            if table.name not in existing_tables:
                continue
            existing_cols = {col["name"] for col in inspector.get_columns(table.name)}
            for column in table.columns:
                if column.name in existing_cols or not column.nullable:
                    continue
                ddl_type = column.type.compile(self.engine.dialect)
                stmt = text(f'ALTER TABLE "{table.name}" ADD COLUMN "{column.name}" {ddl_type}')
                try:
                    with self.engine.begin() as conn:
                        conn.execute(stmt)
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
        return None
    
    db_path = dev_dir / "state.sqlite"
    db = Database(db_path)
    db.ensure_schema_version()
    return db
