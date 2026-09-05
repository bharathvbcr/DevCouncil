"""Rank 10 — honest MCP schemas + bounded list_tasks."""

import json

import pytest
from mcp.types import ListToolsResult

from devcouncil.domain.task import Task
from devcouncil.integrations.mcp.handlers import provenance
from devcouncil.integrations.mcp.server import call_tool, list_tools
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _seed(tmp_path, n=3):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        repo = TaskRepository(session)
        for i in range(n):
            repo.save(Task(id=f"TASK-{i:03d}", title=f"T{i}", description="d",
                           status="blocked" if i == 0 else "planned"))
    return db


@pytest.mark.anyio
async def test_list_tasks_paginates(tmp_path, monkeypatch):
    _seed(tmp_path, n=3)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    payload = json.loads((await call_tool("devcouncil_list_tasks", {"limit": 2, "offset": 0}))[0].text)
    assert payload["total"] == 3
    assert payload["returned"] == 2
    assert len(payload["tasks"]) == 2


@pytest.mark.anyio
async def test_list_tasks_status_filter(tmp_path, monkeypatch):
    _seed(tmp_path, n=3)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    payload = json.loads((await call_tool("devcouncil_list_tasks", {"status": "blocked"}))[0].text)
    assert payload["total"] == 1
    assert payload["tasks"][0]["status"] == "blocked"


@pytest.mark.anyio
async def test_list_tasks_is_compact(tmp_path, monkeypatch):
    _seed(tmp_path, n=1)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    payload = json.loads((await call_tool("devcouncil_list_tasks", {}))[0].text)
    task = payload["tasks"][0]
    assert set(task) <= {"id", "title", "status", "priority", "requirements", "lease"}
    assert task["id"] == "TASK-000"
    assert "requirements" in task


@pytest.mark.anyio
async def test_record_command_rejects_invalid_status(tmp_path, monkeypatch):
    _seed(tmp_path, n=1)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = json.loads((await call_tool("devcouncil_checkout_task", {"task_id": "TASK-000", "client_id": "a"}))[0].text)
    token = checkout["lease_token"]

    bad = json.loads((await call_tool("devcouncil_record_command", {
        "task_id": "TASK-000", "lease_token": token, "command": "pytest", "status": "bogus",
    }))[0].text)
    assert bad["ok"] is False
    assert bad["code"] == "invalid_arguments"

    good = json.loads((await call_tool("devcouncil_record_command", {
        "task_id": "TASK-000", "lease_token": token, "command": "pytest", "status": "finished",
    }))[0].text)
    assert good["ok"] is True


@pytest.mark.anyio
async def test_verify_task_sandbox_enum_is_honest():
    tools = {t.name: t for t in await list_tools()}
    enum = tools["devcouncil_verify_task"].input_schema["properties"]["sandbox"]["enum"]
    assert enum == ["local"]  # docker/nix were advertised but rejected


@pytest.mark.anyio
async def test_ordinary_write_and_command_tools_do_not_require_task_ids():
    tools = {t.name: t for t in await list_tools()}

    assert "task_id" not in tools["devcouncil_write_file"].input_schema["required"]
    assert "task_id" not in tools["devcouncil_apply_patch"].input_schema["required"]
    assert "task_id" not in tools["devcouncil_run_command"].input_schema["required"]
    assert "task_id" in tools["devcouncil_verify_task"].input_schema["required"]


@pytest.mark.anyio
async def test_off_mode_mcp_write_and_command_work_without_task_ids(
    tmp_path,
    monkeypatch,
):
    _seed(tmp_path, n=1)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\ngates:\n  mode: off\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    write = json.loads(
        (
            await call_tool(
                "devcouncil_write_file",
                {"path": "src/taskless.py", "content": "VALUE = 1\n"},
            )
        )[0].text
    )
    command = json.loads(
        (
            await call_tool(
                "devcouncil_run_command",
                {"command": "echo taskless"},
            )
        )[0].text
    )

    assert write["ok"] is True
    assert write["task_id"] is None
    assert command["ok"] is True
    assert command["task_id"] is None
    assert "taskless" in command["stdout"]


# --- wire shape: field names in, camelCase aliases out ---------------------------


@pytest.mark.anyio
async def test_every_advertised_tool_serialises_its_schema_as_inputschema():
    """The handlers build ``Tool`` by field name; the wire must still say ``inputSchema``.

    ``mcp.types`` accepts both spellings (``populate_by_name``) and mypy accepts
    only the field name, so the constructor call is free to change. What is not
    free to change is the payload a client parses: the spec names the key
    ``inputSchema``, and a serialisation that emitted ``input_schema`` would
    leave every tool schemaless without any error on either side.
    """
    payload = ListToolsResult(tools=await list_tools()).model_dump(by_alias=True)
    assert payload["tools"], "no tools advertised"
    wrong = [
        tool.get("name")
        for tool in payload["tools"]
        if "inputSchema" not in tool or "input_schema" in tool
    ]
    assert wrong == [], f"tools serialised without an inputSchema key: {wrong}"


@pytest.mark.anyio
async def test_listed_resources_serialise_their_type_as_mimetype(tmp_path, monkeypatch):
    """Same contract on the resource side: the wire key is ``mimeType``."""
    descriptors = {
        "resources": [
            {
                "uri": "devcouncil://report",
                "name": "report",
                "description": "coverage report",
                "mimeType": "application/json",
            }
        ]
    }
    monkeypatch.setattr(
        provenance,
        "run_cli_command",
        lambda *args, **kwargs: {"ok": True, "stdout": json.dumps(descriptors), "stderr": ""},
    )
    resources = await provenance.list_resources(tmp_path)
    dumped = [resource.model_dump(by_alias=True) for resource in resources]
    assert [item["uri"] for item in dumped] == ["devcouncil://report"]
    assert [item["mimeType"] for item in dumped] == ["application/json"]
    assert all("mime_type" not in item for item in dumped)
