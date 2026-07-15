import json
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap
from devcouncil.storage.repositories import TaskRepository, GapRepository

runner = CliRunner()


def test_cli_status_auto_inits_empty_directory(tmp_path, monkeypatch):
    """status auto-initializes via initialize_project(quiet=True)."""
    monkeypatch.chdir(tmp_path)
    res = runner.invoke(app, ["status"])
    assert res.exit_code == 0
    assert "phase:" in res.output.lower()
    assert (tmp_path / ".devcouncil" / "config.yaml").is_file()


def test_cli_status_json_auto_inits_empty_directory(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    res = runner.invoke(app, ["status", "--json"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["initialized"] is True
    assert data["phase"] == "NEW"


def test_cli_status_initialized(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    # Add a mock task and blocking gap
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(id="TASK-1", title="Task 1", description="D", status="running")
    gap = Gap(
        id="GAP-1",
        severity="high",
        gap_type="missing_test",
        description="Missing test gap",
        blocking=True,
        recommended_fix="Write a test",
        task_id="TASK-1",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        GapRepository(session).save(gap)
        
    res = runner.invoke(app, ["status"])
    assert res.exit_code == 0
    assert "Phase:" in res.output
    assert "WARNING: 1 blocking gap(s) must be resolved:" in res.output
    
    # Test --json flag
    res_json = runner.invoke(app, ["status", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["initialized"] is True
    assert len(data["blocking_gaps"]) == 1
    
    # Test --fail-on-blocking exit code 1
    res_fail = runner.invoke(app, ["status", "--fail-on-blocking"])
    assert res_fail.exit_code == 1


def test_cli_status_fail_on_blocking_no_gaps(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    # Task with no gaps
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(id="TASK-1", title="Task 1", description="D", status="running")
    with db.get_session() as session:
        TaskRepository(session).save(task)
        
    res = runner.invoke(app, ["status", "--fail-on-blocking"])
    assert res.exit_code == 0
