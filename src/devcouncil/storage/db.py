from contextlib import contextmanager

from sqlmodel import SQLModel, create_engine, Session
from sqlalchemy.exc import OperationalError
from pathlib import Path
from typing import Optional

from devcouncil.storage.models import SchemaVersionModel


# v3: per-task indexes on the task/audit tables (task_id, lease status).
SCHEMA_VERSION = 3


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
        self._create_missing_indexes()

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
