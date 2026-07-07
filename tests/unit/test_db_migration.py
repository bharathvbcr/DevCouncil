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


def test_schema_migration_adds_missing_non_nullable_columns_with_default(tmp_path):
    """Regression: v7 added NOT NULL task columns (agent_appended_*_json). The column
    migration used to skip every non-nullable column, so databases created before the
    columns existed crashed EVERY task SELECT (including the agent-response hook) with
    sqlite3.OperationalError: no such column."""
    from sqlalchemy import text

    db_path = tmp_path / "state.sqlite"
    db = Database(db_path)
    db.create_db_and_tables()

    # Simulate the pre-v7 table: rebuild `tasks` without the two new columns.
    with db.engine.begin() as conn:
        conn.execute(text("ALTER TABLE tasks RENAME TO tasks_old"))
        conn.execute(text(
            """
            CREATE TABLE tasks (
                id VARCHAR NOT NULL PRIMARY KEY,
                title VARCHAR NOT NULL,
                description VARCHAR NOT NULL,
                requirement_ids_json VARCHAR NOT NULL,
                acceptance_criterion_ids_json VARCHAR NOT NULL,
                planned_files_json VARCHAR NOT NULL,
                expected_tests_json VARCHAR NOT NULL,
                allowed_commands_json VARCHAR NOT NULL,
                forbidden_changes_json VARCHAR NOT NULL,
                difficulty VARCHAR,
                status VARCHAR NOT NULL
            )
            """
        ))
        conn.execute(text(
            "INSERT INTO tasks (id, title, description, requirement_ids_json, "
            "acceptance_criterion_ids_json, planned_files_json, expected_tests_json, "
            "allowed_commands_json, forbidden_changes_json, difficulty, status) "
            "VALUES ('TASK-1', 't', 'd', '[]', '[]', '[]', '[]', '[]', '[]', NULL, 'planned')"
        ))
        conn.execute(text("DROP TABLE tasks_old"))
        conn.execute(text("UPDATE schema_version SET version = 6"))

    upgraded = Database(db_path)
    upgraded.ensure_schema_version()

    from devcouncil.storage.repositories import TaskRepository

    with upgraded.get_session() as session:
        tasks = TaskRepository(session).get_all()  # crashed before the fix
    assert len(tasks) == 1
    assert tasks[0].agent_appended_expected_tests == []
    assert tasks[0].agent_appended_allowed_commands == []

    with upgraded.engine.connect() as conn:
        version = conn.exec_driver_sql("SELECT version FROM schema_version").fetchone()[0]
    assert version == SCHEMA_VERSION


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
