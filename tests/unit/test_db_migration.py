from devcouncil.storage.db import SCHEMA_VERSION, Database
from devcouncil.storage.models import TaskLeaseModel


def test_schema_migration_adds_missing_indexes_and_preserves_data(tmp_path):
    db_path = tmp_path / "state.sqlite"
    db = Database(db_path)
    db.create_db_and_tables()
    with db.get_session() as session:
        session.add(
            TaskLeaseModel(
                id="LEASE-1",
                task_id="TASK-1",
                owner="tester",
                lease_token="token",
                created_at="2026-01-01T00:00:00+00:00",
            )
        )

    # Simulate an older database: strip the model-defined indexes and
    # downgrade the recorded schema version.
    with db.engine.connect() as conn:
        rows = conn.exec_driver_sql(
            "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'ix_%'"
        ).fetchall()
        assert rows, "expected model-defined indexes in the fresh database"
        for (name,) in rows:
            conn.exec_driver_sql(f'DROP INDEX "{name}"')
        conn.exec_driver_sql("UPDATE schema_version SET version = 2")
        conn.commit()

    upgraded = Database(db_path)
    upgraded.ensure_schema_version()

    with upgraded.engine.connect() as conn:
        names = {
            row[0]
            for row in conn.exec_driver_sql("SELECT name FROM sqlite_master WHERE type='index'")
        }
        version = conn.exec_driver_sql("SELECT version FROM schema_version").fetchone()[0]
    assert "ix_task_leases_task_id" in names
    assert "ix_task_leases_status" in names
    assert version == SCHEMA_VERSION

    with upgraded.get_session() as session:
        lease = session.get(TaskLeaseModel, "LEASE-1")
        assert lease is not None
        assert lease.task_id == "TASK-1"


def test_fresh_database_creates_indexes(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()
    with db.engine.connect() as conn:
        names = {
            row[0]
            for row in conn.exec_driver_sql("SELECT name FROM sqlite_master WHERE type='index'")
        }
    assert "ix_task_leases_task_id" in names
    assert "ix_file_change_events_task_id" in names
