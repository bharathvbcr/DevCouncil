import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _setup_run_db(tmp_path: Path, monkeypatch) -> tuple[Path, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    
    import subprocess
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    
    runner.invoke(app, ["init"])
    
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    from devcouncil.domain.task import PlannedFile
    task = Task(
        id="TASK-1",
        title="Task to run",
        description="d",
        status="planned",
        planned_files=[PlannedFile(path="src/a.py", reason="r", allowed_change="modify")],
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
    return tmp_path, "TASK-1"


def test_cli_run_manual(tmp_path, monkeypatch):
    _setup_run_db(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["run", "TASK-1", "--executor", "manual"])
    assert res.exit_code == 0
    assert "marked as RUNNING" in res.output
    
    # Verify task status in database is now running
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-1")
        assert task.status == "running"


def test_cli_run_unknown_task(tmp_path, monkeypatch):
    _setup_run_db(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["run", "TASK-999"])
    assert "not found" in res.output


def test_cli_run_unknown_executor(tmp_path, monkeypatch):
    _setup_run_db(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["run", "TASK-1", "--executor", "unknown-executor"])
    assert "Unknown executor" in res.output
