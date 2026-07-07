import subprocess

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def test_run_unknown_executor_lists_available(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Sample",
                description="Do work",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python -m pytest"],
                expected_tests=["python -m pytest"],
                status="planned",
            )
        )

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "not-a-real-executor"])
    assert result.exit_code == 0
    assert "Unknown executor 'not-a-real-executor'" in result.output
    assert "Available executors:" in result.output
    assert "manual" in result.output
    assert "claude" in result.output
