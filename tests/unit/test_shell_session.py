from devcouncil.domain.task import Task
from devcouncil.execution.shell_session import GuardedShellSession
from devcouncil.storage.db import Database
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository


def test_command_denied_without_allowlist(tmp_path):
    task = Task(id="TASK-001", title="T", description="D", allowed_commands=["echo ok"])
    session = GuardedShellSession(tmp_path, task)
    decision = session.policy.evaluate_command("pytest", task)
    assert decision.action == "deny"


def test_session_records_events(tmp_path):
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    db.create_db_and_tables()
    with db.get_session() as s:
        TaskRepository(s).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                allowed_commands=["python -c \"print('hello')\""],
            )
        )
    task = Task(
        id="TASK-001",
        title="T",
        description="D",
        allowed_commands=["python -c \"print('hello')\""],
    )
    session = GuardedShellSession(tmp_path, task)
    session.start()
    code = session.run_one("python -c \"print('hello')\"")
    session.finish()
    assert code == 0
    with db.get_session() as s:
        lease = TaskLeaseRepository(s).active_for_task("TASK-001")
        assert lease is None
