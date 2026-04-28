import pytest

from devcouncil.integrations.mcp.server import call_tool, list_tools
from devcouncil.storage.db import Database


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_mcp_lists_graph_context_tool():
    tools = await list_tools()

    assert "devcouncil_graph_context" in {tool.name for tool in tools}


@pytest.mark.anyio
async def test_mcp_graph_context_degrades_when_project_uninitialized(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_graph_context", {"files": ["src/app.py"]})

    assert "not initialized" in result[0].text


@pytest.mark.anyio
async def test_mcp_graph_context_degrades_when_crg_disabled(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_graph_context", {"files": ["src/app.py"]})

    assert "disabled" in result[0].text
