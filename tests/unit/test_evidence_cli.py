import json
import subprocess

from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _git(tmp_path, *args):
    subprocess.check_call(
        ["git", "-c", "user.email=test@example.com", "-c", "user.name=Test", *args],
        cwd=tmp_path,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def _setup_task(tmp_path):
    _git(tmp_path, "init")
    initialize_project(tmp_path, quiet=True)
    tests_dir = tmp_path / "tests"
    tests_dir.mkdir()
    (tests_dir / "test_app.py").write_text("def test_app():\n    assert True\n", encoding="utf-8")
    # Commit everything so the resolver falls back to the task's planned files
    # instead of treating the whole fresh repo as changed.
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-q", "-m", "init")
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Evidence task",
                description="desc",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            )
        )
    return db


def test_evidence_help():
    result = runner.invoke(app, ["evidence", "--help"])
    assert result.exit_code == 0
    assert "suggest" in result.stdout


def test_evidence_suggest_reports_matching_test_with_high_confidence(tmp_path):
    _setup_task(tmp_path)

    result = runner.invoke(app, ["evidence", "suggest", "TASK-001", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["task_id"] == "TASK-001"
    commands = {item["command"]: item for item in payload["suggestions"]}
    assert "pytest tests/test_app.py" in commands
    assert commands["pytest tests/test_app.py"]["confidence"] == "high"
    # No --apply: expected_tests stays untouched.
    assert payload["expected_tests"] == []


def test_evidence_suggest_apply_appends_high_confidence_tests(tmp_path):
    db = _setup_task(tmp_path)

    result = runner.invoke(
        app,
        ["evidence", "suggest", "TASK-001", "--apply", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert "pytest tests/test_app.py" in payload["expected_tests"]
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        assert "pytest tests/test_app.py" in task.expected_tests


def test_evidence_suggest_unknown_task_exits_with_error(tmp_path):
    _git(tmp_path, "init")
    initialize_project(tmp_path, quiet=True)

    result = runner.invoke(app, ["evidence", "suggest", "TASK-404", "--project-root", str(tmp_path)])

    assert result.exit_code == 1
    assert "TASK-404 not found" in result.output
