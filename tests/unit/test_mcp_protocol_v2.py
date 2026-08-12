"""The MCP server speaks the mcp 2.x low-level contract.

mcp 2.0.0 removed the ``@app.list_tools()`` / ``@app.call_tool()`` decorators in
favour of ``Server(on_*=...)`` callables that take ``(ctx, params)`` and return
full protocol results. These tests pin the wire contract -- registration, result
types, inputSchema enforcement, and exception containment -- so the server cannot
regress into the shape that made a fresh install crash at import.
"""

import anyio
import pytest
from mcp import ClientSession
from mcp.shared.memory import create_client_server_memory_streams
from mcp.types import (
    CallToolRequestParams,
    CallToolResult,
    ListToolsResult,
    PaginatedRequestParams,
    ReadResourceRequestParams,
    ReadResourceResult,
)

from devcouncil.integrations.mcp import server


@pytest.fixture
def anyio_backend():
    return "asyncio"


# The adapters ignore the request context, so tests pass None rather than
# fabricating a transport-owned ServerRequestContext.
_NO_CTX = None


@pytest.mark.anyio
async def test_server_registers_every_handler_on_the_v2_dispatch_table():
    for method in (
        "tools/list",
        "tools/call",
        "resources/list",
        "resources/read",
        "prompts/list",
        "prompts/get",
    ):
        assert server.app.get_request_handler(method) is not None, method


@pytest.mark.anyio
async def test_on_list_tools_returns_a_list_tools_result():
    result = await server._on_list_tools(_NO_CTX, PaginatedRequestParams())

    assert isinstance(result, ListToolsResult)
    assert "devcouncil_integration_status" in {tool.name for tool in result.tools}


@pytest.mark.anyio
async def test_on_call_tool_wraps_handler_content_in_a_call_tool_result(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await server._on_call_tool(
        _NO_CTX, CallToolRequestParams(name="devcouncil_integration_status", arguments={})
    )

    assert isinstance(result, CallToolResult)
    assert result.is_error is False
    assert result.content[0].text


@pytest.mark.anyio
async def test_on_call_tool_rejects_arguments_that_violate_the_input_schema(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    # devcouncil_read_file requires a string `path`; 2.x advertises the schema
    # but never enforces it, so the server has to.
    result = await server._on_call_tool(
        _NO_CTX, CallToolRequestParams(name="devcouncil_read_file", arguments={"path": 17})
    )

    assert result.is_error is True
    assert "Input validation error" in result.content[0].text


@pytest.mark.anyio
async def test_on_call_tool_contains_handler_exceptions_as_error_results(monkeypatch):
    async def _boom(name: str, arguments: dict):
        raise RuntimeError("handler exploded")

    monkeypatch.setattr(server, "call_tool", _boom)

    result = await server._on_call_tool(
        _NO_CTX, CallToolRequestParams(name="devcouncil_status", arguments={})
    )

    assert result.is_error is True
    assert "handler exploded" in result.content[0].text


@pytest.mark.anyio
async def test_on_read_resource_returns_text_resource_contents(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / ".devcouncil" / "knowledge" / "design").mkdir(parents=True)
    (tmp_path / ".devcouncil" / "knowledge" / "design" / "design.md").write_text(
        "---\ndescription: Acme design system\n---\n\nPrimary color is #0A66C2.\n",
        encoding="utf-8",
    )

    result = await server._on_read_resource(
        _NO_CTX, ReadResourceRequestParams(uri="devcouncil://knowledge")
    )

    assert isinstance(result, ReadResourceResult)
    assert result.contents[0].uri == "devcouncil://knowledge"
    assert result.contents[0].mime_type == "text/plain"
    assert "Acme design system" in result.contents[0].text


@pytest.mark.anyio
async def test_real_client_session_can_initialize_and_list_tools(tmp_path, monkeypatch):
    """End-to-end over in-memory streams: the failure mode was a dead server."""
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    async with create_client_server_memory_streams() as (client_streams, server_streams):
        client_read, client_write = client_streams
        server_read, server_write = server_streams

        async with anyio.create_task_group() as tg:
            tg.start_soon(
                server.app.run, server_read, server_write, server.app.create_initialization_options()
            )

            async with ClientSession(client_read, client_write) as session:
                await session.initialize()
                listed = await session.list_tools()
                called = await session.call_tool("devcouncil_integration_status", {})

            tg.cancel_scope.cancel()

    assert "devcouncil_integration_status" in {tool.name for tool in listed.tools}
    assert called.is_error is False
