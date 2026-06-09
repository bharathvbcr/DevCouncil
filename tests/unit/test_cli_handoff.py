import json
import subprocess

from sqlmodel import select
from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.models import AgentHandoffModel
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _setup_task(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    initialize_project(tmp_path, quiet=True)
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Handoff task",
                description="desc",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            )
        )
    return db


def test_handoff_help():
    result = runner.invoke(app, ["handoff", "--help"])
    assert result.exit_code == 0


def test_handoff_writes_manifest_and_records_handoff(tmp_path):
    db = _setup_task(tmp_path)

    result = runner.invoke(
        app,
        [
            "handoff",
            "TASK-001",
            "--from",
            "codex",
            "--to",
            "aider",
            "--instruction",
            "Continue the fix",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    manifests = list((tmp_path / ".devcouncil" / "runs").glob("*/handoff.json"))
    assert len(manifests) == 1
    data = json.loads(manifests[0].read_text(encoding="utf-8"))
    assert data["from_agent"] == "codex"
    assert data["to_agent"] == "aider"
    assert data["instruction"] == "Continue the fix"
    assert data["task"]["id"] == "TASK-001"
    assert data["checkpoint_refs"]["before"] == "refs/devcouncil/tasks/TASK-001/before"

    with db.get_session() as session:
        handoffs = session.exec(select(AgentHandoffModel)).all()
        assert len(handoffs) == 1
        assert handoffs[0].from_agent == "codex"
        assert handoffs[0].to_agent == "aider"
        assert handoffs[0].status == "manifest_only"


def test_handoff_rejects_unknown_agent(tmp_path):
    _setup_task(tmp_path)

    result = runner.invoke(
        app,
        ["handoff", "TASK-001", "--from", "codex", "--to", "not-a-real-agent", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 1
    assert "Unknown agent name" in result.output


def test_handoff_unknown_task_exits_with_error(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    initialize_project(tmp_path, quiet=True)

    result = runner.invoke(
        app,
        ["handoff", "TASK-404", "--from", "codex", "--to", "aider", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 1
    assert "TASK-404 not found" in result.output
