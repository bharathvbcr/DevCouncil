"""Rank 18 — DevCouncil corpus is browsable as MCP resources."""

import json

import pytest
from pydantic import AnyUrl

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.integrations.mcp.server import list_resources, read_resource
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _seed(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="Build it", description="d"))
        GapRepository(session).save(Gap(
            id="GAP-1", severity="high", gap_type="test_failed", task_id="TASK-001",
            description="boom", recommended_fix="fix", blocking=True,
        ))


@pytest.mark.anyio
async def test_list_resources_includes_corpus_and_tasks(tmp_path, monkeypatch):
    _seed(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    uris = {str(r.uri).rstrip("/") for r in await list_resources()}
    assert "devcouncil://report" in uris
    assert "devcouncil://tasks" in uris
    assert "devcouncil://gaps" in uris
    assert "devcouncil://task/TASK-001" in uris


@pytest.mark.anyio
async def test_read_resource_tasks_and_task_detail(tmp_path, monkeypatch):
    _seed(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    tasks = json.loads(await read_resource(AnyUrl("devcouncil://tasks")))
    assert tasks["tasks"][0]["id"] == "TASK-001"

    detail = json.loads(await read_resource(AnyUrl("devcouncil://task/TASK-001")))
    assert detail["task"]["id"] == "TASK-001"
    assert detail["gaps"][0]["id"] == "GAP-1"

    gaps = json.loads(await read_resource(AnyUrl("devcouncil://gaps")))
    assert any(g["id"] == "GAP-1" for g in gaps["gaps"])


@pytest.mark.anyio
async def test_read_resource_report_is_markdown(tmp_path, monkeypatch):
    _seed(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    report = await read_resource(AnyUrl("devcouncil://report"))
    assert isinstance(report, str) and report.strip()


@pytest.mark.anyio
async def test_read_resource_unknown_uri_raises(tmp_path, monkeypatch):
    _seed(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with pytest.raises(ValueError):
        await read_resource(AnyUrl("devcouncil://nope"))
