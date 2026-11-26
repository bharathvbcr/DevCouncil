"""Unit tests for Antigravity MCP server configuration and the Waylode (formerly GitPulse) A2A allowlist."""

import asyncio
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

if "antigravity" in sys.modules and not hasattr(sys.modules["antigravity"], "__path__"):
    del sys.modules["antigravity"]

from antigravity.config import mcp_bridge
from antigravity.config.agent_config import (
    GITPULSE_TOOLS,
    WAYLODE_ALLOWLIST,
    WAYLODE_TOOLS,
    McpStdioServer,
    create_unified_antigravity_config,
    validate_fastmcp_identifier,
)
from antigravity.config.mcp_bridge import (
    call_mcp_tool,
    clear_in_process_handlers,
    register_in_process_handler,
)

A2A_TOOLS = (
    "waylode_agent_register",
    "waylode_agent_peers",
    "waylode_agent_send",
    "waylode_agent_inbox",
    "waylode_agent_wait",
)
OLD_A2A_TOOLS = tuple(name.replace("waylode_", "gitpulse_", 1) for name in A2A_TOOLS)


def _yaml_waylode_tools(yaml_text: str) -> list[str]:
    in_server = False
    in_tools = False
    items: list[str] = []
    for raw in yaml_text.splitlines():
        if raw.startswith("  waylode:"):
            in_server = True
            continue
        if in_server and raw.startswith("  ") and not raw.startswith("    "):
            break
        if in_server and raw.strip() == "tools:":
            in_tools = True
            continue
        if in_tools:
            stripped = raw.strip()
            if stripped.startswith("- "):
                items.append(stripped[2:].strip())
            elif stripped and not stripped.startswith("#"):
                break
    return items


def test_waylode_a2a_tools_unprefixed_and_synced_across_all_manifests():
    config_dir = Path(__file__).resolve().parent
    json_tools = json.loads((config_dir / "mcp_config.json").read_text())["mcpServers"][
        "waylode"
    ]["tools"]
    yaml_tools = _yaml_waylode_tools((config_dir / "mcp_config.yaml").read_text())

    for tool in A2A_TOOLS + OLD_A2A_TOOLS:
        assert validate_fastmcp_identifier(tool)
        assert not tool.startswith("mcp__")
        assert tool in WAYLODE_ALLOWLIST
    for tool in OLD_A2A_TOOLS:
        assert tool in GITPULSE_TOOLS

    # Both spellings, new first, so a host started before the rename still matches.
    assert WAYLODE_ALLOWLIST == WAYLODE_TOOLS + GITPULSE_TOOLS
    assert json_tools == WAYLODE_ALLOWLIST
    assert yaml_tools == WAYLODE_ALLOWLIST
    assert len(set(WAYLODE_ALLOWLIST)) == len(WAYLODE_ALLOWLIST)
    assert len(WAYLODE_TOOLS) == len(GITPULSE_TOOLS) == 23

    cfg = create_unified_antigravity_config(workspace_root=str(REPO_ROOT))
    server = cfg.get_server("waylode")
    assert server is not None
    assert server.enabled_tools == WAYLODE_ALLOWLIST
    # The pre-rename key still resolves, to the same server rather than a second one.
    assert cfg.get_server("gitpulse") is server
    assert [s.name for s in cfg.mcp_servers].count("waylode") == 1


def test_mcp_server_rejects_malformed_tool_identifiers_and_bridge_routes_a2a():
    with pytest.raises(ValueError):
        McpStdioServer(
            name="waylode",
            command="gitpulse-mcp",
            enabled_tools=["waylode:agent-send"],
        )

    seen: list[tuple[str, dict]] = []

    def handler(tool_name: str, args: dict) -> dict:
        seen.append((tool_name, args))
        return {"ok": True, "tool": tool_name}

    register_in_process_handler("waylode", handler)
    try:
        for key in ("waylode", "gitpulse"):
            for tool in A2A_TOOLS + OLD_A2A_TOOLS:
                res = asyncio.run(call_mcp_tool(key, tool, {"repo_path": str(REPO_ROOT)}))
                assert res["ok"] is True
                # A handler registered under the new key is always handed the new spelling.
                assert res["tool"] == tool.replace("gitpulse_", "waylode_", 1)
        assert {name for name, _ in seen} == set(A2A_TOOLS)
    finally:
        clear_in_process_handlers()


def test_handler_registered_under_the_old_key_is_handed_the_old_spelling():
    seen: list[str] = []

    def old_handler(tool_name: str, args: dict) -> dict:
        seen.append(tool_name)
        return {"ok": True}

    register_in_process_handler("gitpulse", old_handler)
    try:
        asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
        asyncio.run(call_mcp_tool("gitpulse", "waylode_collision_risk", {}))
        assert seen == ["gitpulse_collision_risk", "gitpulse_collision_risk"]
    finally:
        clear_in_process_handlers()


@pytest.fixture
def no_server(monkeypatch):
    """No in-process handler, no binary: the offline answers are all there is."""
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: None)


@pytest.mark.parametrize("server", ["waylode", "gitpulse"])
@pytest.mark.parametrize(
    "tool", ["waylode_collision_risk", "gitpulse_collision_risk"]
)
def test_offline_collision_scan_answers_under_either_key_and_spelling(no_server, server, tool):
    res = asyncio.run(call_mcp_tool(server, tool, {"repo_path": str(REPO_ROOT)}))
    assert res == {"ok": True, "status": "clean", "overlapping_files": []}


@pytest.mark.parametrize(
    ("server", "tool"),
    [
        ("waylode", "waylode_collision_rsk"),  # misspelled
        ("waylode", "waylode_not_a_tool"),
        ("gitpulse", "gitpulse_not_a_tool"),
        ("waylode", "collision_risk"),  # lost its prefix
        ("devmap", "devmap_not_a_tool"),
        ("devcouncil", "devcouncil_not_a_tool"),
    ],
)
def test_an_unrecognised_tool_fails_closed_instead_of_reading_as_success(no_server, server, tool):
    # Regression: the offline fallthrough used to answer
    # {"ok": True, "simulated": True, "tool": ...} for ANY name, so a misspelled or
    # renamed tool looked like success.
    res = asyncio.run(call_mcp_tool(server, tool, {"repo_path": str(REPO_ROOT)}))
    assert res["ok"] is False
    assert res.get("unknown_tool") is True
    assert "simulated" not in res
    assert tool in res["error"]


def test_stdio_unknown_tool_is_retried_under_the_other_spelling(monkeypatch):
    # A server from before the rename lists gitpulse_*; one from after lists waylode_*.
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])
    called: list[str] = []

    async def fake_exec(cmd, tool_name, arguments):
        called.append(tool_name)
        if tool_name.startswith("waylode_"):
            return {"ok": False, "error": {"code": -32602, "message": f'unknown tool "{tool_name}"'}}
        return {"ok": True, "overlapping_files": [{"path": "src/a.rs"}]}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", fake_exec)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert called == ["waylode_collision_risk", "gitpulse_collision_risk"]
    # The real answer came back, not the offline "clean" one.
    assert res == {"ok": True, "overlapping_files": [{"path": "src/a.rs"}]}


def test_stdio_unknown_under_both_spellings_is_an_error_not_an_offline_clean(monkeypatch):
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])

    async def fake_exec(cmd, tool_name, arguments):
        return {"ok": False, "error": f'unknown tool "{tool_name}"'}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", fake_exec)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert res["unknown_tool"] is True
    assert "waylode_collision_risk" in res["error"] and "gitpulse_collision_risk" in res["error"]


def test_a_server_that_answered_with_an_error_is_not_replaced_by_an_offline_clean(monkeypatch):
    # The server was reached and refused the call. Falling through to the offline
    # simulation would turn that refusal into a clean collision scan.
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])

    async def fake_exec(cmd, tool_name, arguments):
        return {"ok": False, "error": "invalid arguments for gitpulse_collision_risk", "server_answered": True}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", fake_exec)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert "invalid arguments" in res["error"]


def test_an_unreachable_server_still_gets_the_offline_answer(monkeypatch):
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])

    async def fake_exec(cmd, tool_name, arguments):
        return {"ok": False, "error": "[Errno 2] No such file or directory: 'fake-mcp'"}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", fake_exec)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert res["ok"] is True and res["status"] == "clean"


def test_an_mcp_tool_error_reply_is_an_error_not_ok_output():
    # Regression: {"result": {"isError": true, "content": [...]}} was returned as
    # {"ok": True, "output": <the error text>}.
    reply = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 2,
            "result": {"isError": True, "content": [{"type": "text", "text": "invalid arguments: agent_id"}]},
        }
    )
    cmd = [sys.executable, "-I", "-c", f"import sys; sys.stdin.read(); print({reply!r})"]
    res = asyncio.run(mcp_bridge._exec_mcp_stdio_jsonrpc(cmd, "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert res["server_answered"] is True
    assert "invalid arguments" in res["error"]


def _fake_server(reply: dict) -> list[str]:
    """A command that answers any stdin with one tools/call reply carrying `reply`."""
    message = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 2,
            "result": {"isError": False, "content": [{"type": "text", "text": json.dumps(reply)}]},
        }
    )
    return [sys.executable, "-I", "-c", f"import sys; sys.stdin.read(); print({message!r})"]


def test_an_ok_false_body_from_the_server_is_never_replaced_by_an_offline_clean(monkeypatch):
    # The server's documented answer when a worktree could not be read is a normal
    # reply whose JSON body says ok: false -- no isError, no JSON-RPC error. It must
    # reach the caller, not be swapped for the offline "clean".
    clear_in_process_handlers()
    refusal = {"ok": False, "error": "worktree unreadable", "truncated": True, "overlapping_files": 0}
    cmd = _fake_server(refusal)
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: cmd)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert res["error"] == "worktree unreadable"
    assert res.get("status") != "clean"


def test_a_server_that_never_answers_tools_call_is_not_an_empty_success():
    cmd = [sys.executable, "-I", "-c", "import sys; sys.stdin.read()"]
    res = asyncio.run(mcp_bridge._exec_mcp_stdio_jsonrpc(cmd, "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert "no tools/call reply" in res["error"]


def test_resolve_binary_command_prefers_waylode_mcp_and_falls_back_to_gitpulse_mcp(monkeypatch):
    monkeypatch.setattr(
        mcp_bridge.shutil,
        "which",
        lambda name: f"/usr/local/bin/{name}" if name in ("waylode-mcp", "gitpulse-mcp") else None,
    )
    assert mcp_bridge._resolve_binary_command("waylode") == ["/usr/local/bin/waylode-mcp"]
    assert mcp_bridge._resolve_binary_command("gitpulse") == ["/usr/local/bin/waylode-mcp"]

    monkeypatch.setattr(
        mcp_bridge.shutil,
        "which",
        lambda name: "/usr/local/bin/gitpulse-mcp" if name == "gitpulse-mcp" else None,
    )
    assert mcp_bridge._resolve_binary_command("waylode") == ["/usr/local/bin/gitpulse-mcp"]
    assert mcp_bridge._resolve_binary_command("gitpulse") == ["/usr/local/bin/gitpulse-mcp"]
