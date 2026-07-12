import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _setup_show_db(tmp_path: Path, monkeypatch) -> tuple[Path, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Original Title",
        description="Original Desc",
        status="planned",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
    return tmp_path, "TASK-1"


def test_cli_show_default(tmp_path, monkeypatch):
    _setup_show_db(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["show", "TASK-1"])
    assert res.exit_code == 0
    assert "Original Title" in res.output
    assert "Original Desc" in res.output


def test_cli_show_json(tmp_path, monkeypatch):
    _setup_show_db(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["show", "TASK-1", "--json"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["task"]["id"] == "TASK-1"
    assert data["task"]["title"] == "Original Title"


def test_cli_show_not_found(tmp_path, monkeypatch):
    _setup_show_db(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["show", "TASK-999"])
    assert res.exit_code != 0
    assert "not found" in res.output.lower()
