import sys

from devcouncil.domain.task import Task
from devcouncil.execution.shell_session import GuardedShellSession
from devcouncil.storage.db import Database
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository

# Interpreter actually running the tests — a bare "python" is not on every host
# (some only ship "python3"), which would fail this command for an unrelated reason.
_PY = sys.executable
_HELLO = f"{_PY} -c \"print('hello')\""


def test_command_denied_without_allowlist(tmp_path):
    task = Task(id="TASK-001", title="T", description="D", allowed_commands=["echo ok"])
    session = GuardedShellSession(tmp_path, task)
    decision = session.policy.evaluate_command(
        "pytest",
        task,
        enforce_task_scope=session.gate_mode == "enforce",
    )
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
                allowed_commands=[_HELLO],
            )
        )
    task = Task(
        id="TASK-001",
        title="T",
        description="D",
        allowed_commands=[_HELLO],
    )
    session = GuardedShellSession(tmp_path, task)
    session.start()
    code = session.run_one(_HELLO)
    session.finish()
    assert code == 0
    with db.get_session() as s:
        lease = TaskLeaseRepository(s).active_for_task("TASK-001")
        assert lease is None


def test_off_mode_shell_needs_no_lease_or_task_allowlist(tmp_path):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    (dev_dir / "config.yaml").write_text(
        "project:\n  name: test\ngates:\n  mode: off\n",
        encoding="utf-8",
    )
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    task = Task(id="TASK-001", title="T", description="D")
    with db.get_session() as session:
        TaskRepository(session).save(task)

    shell = GuardedShellSession(tmp_path, task)
    shell.start()
    code = shell.run_one(_HELLO)
    shell.finish()

    assert code == 0
    with db.get_session() as session:
        assert TaskLeaseRepository(session).active_for_task(task.id) is None
        assert TaskRepository(session).get_by_id(task.id).status == "planned"
