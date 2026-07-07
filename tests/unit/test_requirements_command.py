import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.requirement import Requirement, AcceptanceCriterion
from devcouncil.domain.task import Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import RequirementRepository, TaskRepository

runner = CliRunner()


def test_requirements_command_lists_requirements(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(
            Requirement(
                id="REQ-001",
                title="Auth",
                description="User authentication",
                priority="high",
                acceptance_criteria=[
                    AcceptanceCriterion(
                        id="AC-1",
                        description="Login works",
                        verification_method="unit_test",
                    )
                ],
            )
        )
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="Implement auth",
                description="Build login",
                requirement_ids=["REQ-001"],
                status="running",
            )
        )
        TaskRepository(session).save(
            Task(
                id="TASK-002",
                title="Unlinked req",
                description="Other",
                requirement_ids=["REQ-999"],
                status="planned",
            )
        )

    result = runner.invoke(app, ["requirements", "--json"])
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["total_count"] == 1
    row = payload["requirements"][0]
    assert row["id"] == "REQ-001"
    assert row["priority"] == "high"
    assert row["status"] == "in_progress"
    assert row["linked_task_count"] == 1


def test_requirements_command_shows_unmapped_status(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(
            Requirement(
                id="REQ-002",
                title="Orphan requirement",
                description="No tasks yet",
                priority="medium",
            )
        )

    result = runner.invoke(app, ["requirements", "--json"])
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["requirements"][0]["status"] == "unmapped"
    assert payload["requirements"][0]["linked_task_count"] == 0
