import json

import pytest

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.integrations.mcp.server import call_tool, list_tools
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_checkout_returns_prompt_scope_and_lease_token(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-001", title="T", description="D", allowed_commands=["pytest"])
        )
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool(
        "devcouncil_checkout_task",
        {"task_id": "TASK-001", "client_id": "cursor"},
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["lease_token"]
    assert payload["prompt"]
    assert payload["planned_files"] == []
    assert payload["allowed_commands"] == ["pytest"]


@pytest.mark.anyio
async def test_checkout_rejects_second_active_lease(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"})
    second = await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "b"})
    payload = json.loads(second[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "lease_conflict"


@pytest.mark.anyio
async def test_release_rejects_wrong_token(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )
    released = await call_tool(
        "devcouncil_release_task",
        {"task_id": "TASK-001", "lease_token": "bad-token"},
    )
    payload = json.loads(released[0].text)
    assert payload["ok"] is False

    ok_release = await call_tool(
        "devcouncil_release_task",
        {"task_id": "TASK-001", "lease_token": checkout["lease_token"]},
    )
    assert json.loads(ok_release[0].text)["ok"] is True


@pytest.mark.anyio
async def test_append_evidence_requires_valid_token(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    denied = await call_tool(
        "devcouncil_append_evidence",
        {
            "task_id": "TASK-001",
            "lease_token": "bad",
            "command": "pytest",
            "exit_code": 0,
            "summary": "ok",
        },
    )
    assert json.loads(denied[0].text)["ok"] is False

    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )
    allowed = await call_tool(
        "devcouncil_append_evidence",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "command": "pytest",
            "exit_code": 0,
            "summary": "ok",
        },
    )
    assert json.loads(allowed[0].text)["ok"] is True


@pytest.mark.anyio
async def test_mcp_lists_checkout_tools():
    tools = {tool.name for tool in await list_tools()}
    assert "devcouncil_checkout_task" in tools
    assert "devcouncil_release_task" in tools


@pytest.mark.anyio
async def test_checkout_returns_not_initialized_without_db(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool(
        "devcouncil_checkout_task",
        {"task_id": "TASK-001", "client_id": "cursor"},
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "not_initialized"


@pytest.mark.anyio
async def test_update_scope_rejects_non_array_values(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    result = await call_tool(
        "devcouncil_update_task_scope",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "allowed_commands": "pytest",
        },
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["argument"] == "allowed_commands"


@pytest.mark.anyio
async def test_mcp_verify_persists_gaps_evidence_and_task_status(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    async def fake_verify(self, task, requirements):
        return [
            Gap(
                id="GAP-1",
                severity="high",
                gap_type="test_failed",
                task_id=task.id,
                description="failed",
                recommended_fix="fix",
                blocking=True,
            )
        ], [
            CommandResult(
                command="pytest",
                exit_code=1,
                stdout_path="",
                stderr_path="",
                summary="failed",
            )
        ]

    monkeypatch.setattr("devcouncil.verification.verifier.Verifier.verify_task", fake_verify)
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    result = await call_tool(
        "devcouncil_verify_task",
        {"task_id": "TASK-001", "lease_token": checkout["lease_token"]},
    )

    payload = json.loads(result[0].text)
    assert payload["passed"] is False
    assert payload["status"] == "blocked"
    with db.get_session() as session:
        assert TaskRepository(session).get_by_id("TASK-001").status == "blocked"
        assert GapRepository(session).get_all()[0].id == "GAP-1"
        assert EvidenceRepository(session).get_command_results_for_task("TASK-001")[0].command == "pytest"
