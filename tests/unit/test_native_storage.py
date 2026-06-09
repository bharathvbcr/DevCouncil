
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import Database, SCHEMA_VERSION
from devcouncil.storage.models import SchemaVersionModel, TaskModel
from devcouncil.storage.native import (
    AgentHandoffRepository,
    CorrectionManifestRepository,
    FileChangeRepository,
    SemanticDiffRepository,
    ShellCommandRepository,
    ShellSessionRepository,
    TaskLeaseRepository,
    VerificationRunRepository,
)
from devcouncil.storage.repositories import TaskRepository


def _seed_task(db: Database, task_id: str = "TASK-001") -> None:
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id=task_id,
                title="Test",
                description="Test task",
                planned_files=[
                    PlannedFile(path="src/app.py", reason="impl", allowed_change="modify"),
                ],
            )
        )


def test_schema_migrates_v1_to_v2_without_losing_tasks(tmp_path):
    db_path = tmp_path / "state.sqlite"
    db = Database(db_path)
    db._create_tables()
    with db.get_session() as session:
        session.add(SchemaVersionModel(id="singleton", version=1))
        session.add(
            TaskModel(
                id="TASK-LEGACY",
                title="Legacy",
                description="Keep me",
                status="planned",
            )
        )

    db.ensure_schema_version()
    with db.get_session() as session:
        version = session.get(SchemaVersionModel, "singleton")
        assert version is not None
        assert version.version == SCHEMA_VERSION
        task = session.get(TaskModel, "TASK-LEGACY")
        assert task is not None
        assert task.title == "Legacy"


def test_task_lease_acquire_rejects_existing_active_lease(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()
    _seed_task(db)

    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        repo.acquire("TASK-001", "owner-a")
        try:
            repo.acquire("TASK-001", "owner-b")
            assert False, "expected ValueError"
        except ValueError as exc:
            assert "Active lease" in str(exc)


def test_task_lease_force_replaces_existing_active_lease(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()
    _seed_task(db)

    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        first = repo.acquire("TASK-001", "owner-a")
        second = repo.acquire("TASK-001", "owner-b", force=True)
        assert second.lease_token != first.lease_token
        active = repo.active_for_task("TASK-001")
        assert active is not None
        assert active.owner == "owner-b"


def test_task_lease_release_requires_matching_token(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()
    _seed_task(db)

    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.acquire("TASK-001", "owner-a")
        assert repo.release("TASK-001", "wrong-token") is False
        assert repo.release("TASK-001", lease.lease_token) is True
        assert repo.active_for_task("TASK-001") is None


def test_native_event_repositories_round_trip_json_payloads(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()
    _seed_task(db)

    with db.get_session() as session:
        lease = TaskLeaseRepository(session).acquire("TASK-001", "shell")
        shell = ShellSessionRepository(session).start(
            "TASK-001", "bash", str(tmp_path), lease_id=lease.id
        )
        ShellSessionRepository(session).finish(shell.id, "finished")

        cmd = ShellCommandRepository(session).record(
            "TASK-001",
            "pytest tests/",
            "finished",
            session_id=shell.id,
            exit_code=0,
            reason="ok",
        )
        assert cmd.status == "finished"

        change = FileChangeRepository(session).record(
            "src/app.py",
            "modify",
            True,
            task_id="TASK-001",
            lease_id=lease.id,
            reason="planned",
        )
        assert change.allowed is True

        classifications = [{"type": "public_api_change", "path": "src/app.py"}]
        diff = SemanticDiffRepository(session).save(
            "TASK-001",
            ".devcouncil/semantic/TASK-001/before.json",
            ".devcouncil/semantic/TASK-001/after.json",
            classifications,
            "API changed",
        )
        assert diff.classifications == classifications

        handoff = AgentHandoffRepository(session).save(
            "TASK-001",
            "codex",
            "aider",
            "run-1",
            ".devcouncil/runs/run-1/handoff.json",
            "ready",
        )
        assert handoff.from_agent == "codex"

        correction = CorrectionManifestRepository(session).save(
            "TASK-001",
            ".devcouncil/runs/run-1/correction-manifest.json",
            "open",
            retry_budget=2,
            attempt=1,
        )
        assert correction.retry_budget == 2

        env = {"python": "3.12"}
        commands = [{"command": "pytest", "exit_code": 0}]
        run = VerificationRunRepository(session).save(
            "TASK-001",
            "local",
            env,
            commands,
            "passed",
        )
        assert run.environment == env
        assert run.commands == commands
