import json
from pathlib import Path

from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from typer.testing import CliRunner

from devcouncil.cli.main import app


def _init_with_task(tmp_path: Path) -> str:
    initialize_project(tmp_path, quiet=True)
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        from devcouncil.domain.task import Task

        task = Task(id="TASK-001", title="Original", description="Do the thing")
        TaskRepository(session).save(task)
    return "TASK-001"


def test_tasks_reprioritize_and_edit(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    task_id = _init_with_task(tmp_path)
    runner = CliRunner()

    result = runner.invoke(app, ["tasks", "reprioritize", task_id, "--priority", "high", "--json"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["ok"] is True
    assert payload["priority"] == "high"

    result = runner.invoke(
        app,
        ["tasks", "edit", task_id, "--title", "Updated title", "--description", "New desc", "--json"],
    )
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["ok"] is True
    assert "title" in payload["changes"]

    db = get_db(tmp_path)
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
        assert task is not None
        assert task.priority == "high"
        assert task.title == "Updated title"
        assert task.description == "New desc"


def test_tasks_reprioritize_rejects_invalid_priority(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    task_id = _init_with_task(tmp_path)
    runner = CliRunner()
    result = runner.invoke(app, ["tasks", "reprioritize", task_id, "--priority", "urgent", "--json"])
    assert result.exit_code == 2
