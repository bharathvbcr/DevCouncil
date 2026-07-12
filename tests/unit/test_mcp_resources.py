"""Coverage for reporting.mcp_resources (resource read/list helpers)."""

from __future__ import annotations

import json

import pytest

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.reporting.mcp_resources import list_mcp_resource_uris, read_mcp_resource
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository


@pytest.fixture
def project(tmp_path):
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    return tmp_path


def _add_task(tmp_path, task_id="TASK-001"):
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id=task_id,
                title="A task",
                description="desc",
                planned_files=[PlannedFile(path="src/app.py", reason="x", allowed_change="modify")],
            )
        )


def test_read_report_resource(project):
    body = read_mcp_resource(project, "devcouncil://report")
    assert isinstance(body, str)
    assert body.strip()  # markdown report


def test_read_tasks_empty(project):
    body = read_mcp_resource(project, "devcouncil://tasks")
    data = json.loads(body)
    assert data == {"tasks": []}


def test_read_tasks_with_task(project):
    _add_task(project)
    data = json.loads(read_mcp_resource(project, "devcouncil://tasks"))
    ids = {t["id"] for t in data["tasks"]}
    assert "TASK-001" in ids


def test_read_gaps_empty(project):
    data = json.loads(read_mcp_resource(project, "devcouncil://gaps"))
    assert data == {"gaps": []}


def test_read_cards_resource(project):
    data = json.loads(read_mcp_resource(project, "devcouncil://cards"))
    assert isinstance(data, dict)


def test_read_specific_task_resource(project):
    _add_task(project, "TASK-XYZ")
    data = json.loads(read_mcp_resource(project, "devcouncil://task/TASK-XYZ"))
    assert data["task"]["id"] == "TASK-XYZ"
    assert "gaps" in data


def test_read_missing_task_resource(project):
    data = json.loads(read_mcp_resource(project, "devcouncil://task/NOPE"))
    assert data["ok"] is False
    assert "not found" in data["error"].lower()


def test_read_knowledge_index_empty(project):
    body = read_mcp_resource(project, "devcouncil://knowledge")
    assert "No OKF" in body or "knowledge" in body.lower()


def test_read_unknown_resource_raises(project):
    with pytest.raises(ValueError):
        read_mcp_resource(project, "devcouncil://bogus")


def test_trailing_slash_normalized(project):
    data = json.loads(read_mcp_resource(project, "devcouncil://tasks/"))
    assert data == {"tasks": []}


def test_list_resource_uris_includes_core_and_tasks(project):
    _add_task(project, "TASK-LIST")
    resources = list_mcp_resource_uris(project)
    uris = {r["uri"] for r in resources}
    assert "devcouncil://report" in uris
    assert "devcouncil://tasks" in uris
    assert "devcouncil://gaps" in uris
    assert "devcouncil://cards" in uris
    assert "devcouncil://task/TASK-LIST" in uris
    # each descriptor has the expected shape
    for r in resources:
        assert {"uri", "name", "description", "mimeType"} <= set(r)
