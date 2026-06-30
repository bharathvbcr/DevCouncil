import subprocess
import sys

from sqlmodel import select
from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.models import ShellCommandEventModel, ShellSessionModel, TaskLeaseModel
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()

# Use the interpreter actually running the tests — a bare "python" is not present on
# every host (e.g. systems that only install "python3"), which would otherwise make the
# command exit non-zero for an environment reason unrelated to what these tests check.
_PY = sys.executable


def _setup_task(tmp_path, allowed_commands):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    initialize_project(tmp_path, quiet=True)
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Shell task",
                description="desc",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=allowed_commands,
            )
        )
    return db


def test_shell_registered_in_help():
    result = runner.invoke(app, ["--help"])
    assert result.exit_code == 0
    assert "shell" in result.stdout


def test_shell_runs_allowed_command_and_records_session(tmp_path):
    db = _setup_task(tmp_path, allowed_commands=[f"{_PY} --version"])

    result = runner.invoke(
        app,
        ["shell", "TASK-001", "--command", f"{_PY} --version", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    with db.get_session() as session:
        events = session.exec(select(ShellCommandEventModel)).all()
        assert len(events) == 1
        assert events[0].command == f"{_PY} --version"
        assert events[0].status == "finished"
        assert events[0].exit_code == 0
        shell_sessions = session.exec(select(ShellSessionModel)).all()
        assert len(shell_sessions) == 1
        assert shell_sessions[0].status == "finished"
        leases = session.exec(select(TaskLeaseModel)).all()
        assert len(leases) == 1
        assert leases[0].status == "released"


def test_shell_denies_command_outside_allowlist(tmp_path):
    db = _setup_task(tmp_path, allowed_commands=["python --version"])

    result = runner.invoke(
        app,
        ["shell", "TASK-001", "--command", "curl http://evil.example.com", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 1
    with db.get_session() as session:
        events = session.exec(select(ShellCommandEventModel)).all()
        assert len(events) == 1
        assert events[0].status == "denied"
        assert "allowlist" in events[0].reason


def test_shell_unknown_task_exits_with_error(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    initialize_project(tmp_path, quiet=True)

    result = runner.invoke(
        app,
        ["shell", "TASK-404", "--command", "python --version", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 1
    assert "TASK-404 not found" in result.output
