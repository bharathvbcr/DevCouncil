"""CLI coverage for `dev provenance` and `dev resource` (MCP corpus reads)."""

import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _seed_task(root, task_id="TASK-001"):
    db = get_db(root)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id=task_id,
                title="Task",
                description="desc",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                expected_tests=["pytest"],
                allowed_commands=["pytest"],
            )
        )


def test_provenance_human_for_known_task(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)

    result = runner.invoke(app, ["provenance", "TASK-001"])
    assert result.exit_code == 0
    assert "TASK-001" in result.output
    assert "file change(s)" in result.output


def test_provenance_json_for_known_task(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)

    result = runner.invoke(app, ["provenance", "TASK-001", "--json"])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["ok"] is True
    assert "file_changes" in data


def test_provenance_json_honors_project_root(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    project = tmp_path / "proj"
    project.mkdir()
    monkeypatch.chdir(tmp_path)
    initialize_project(project)
    _seed_task(project)

    result = runner.invoke(app, ["provenance", "TASK-001", "--json", "--project-root", str(project)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["ok"] is True


def test_resource_list_human(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["resource", "list", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "devcouncil://tasks" in result.output


def test_resource_list_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["resource", "list", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    uris = {r["uri"] for r in json.loads(result.stdout)["resources"]}
    assert "devcouncil://tasks" in uris


def test_resource_read_tasks(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _seed_task(tmp_path)

    result = runner.invoke(app, ["resource", "read", "devcouncil://tasks", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "TASK-001" in result.output


def test_resource_read_unknown_uri_errors(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["resource", "read", "devcouncil://nope", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
