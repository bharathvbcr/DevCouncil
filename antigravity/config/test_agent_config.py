"""Unit tests for Antigravity MCP server configuration, tool allowlists, and fail-closed MCP bridge."""

import asyncio
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

if "antigravity" in sys.modules and not hasattr(sys.modules["antigravity"], "__path__"):
    del sys.modules["antigravity"]

from antigravity.config import mcp_bridge
from antigravity.config.agent_config import (
    DEVCOUNCIL_TOOLS,
    DEVMAP_TOOLS,
    GITPULSE_TOOLS,
    WAYLODE_ALLOWLIST,
    WAYLODE_TOOLS,
    McpStdioServer,
    McpStreamableHttpServer,
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

EXPECTED_21_DEVMAP_TOOLS = [
    "devmap_status",
    "devmap_search",
    "devmap_ask",
    "devmap_ask_evidence",
    "devmap_dependencies",
    "devmap_impact",
    "devmap_trace",
    "devmap_neighbors",
    "devmap_dead_symbols",
    "devmap_skeleton",
    "devmap_clones",
    "devmap_preview",
    "devmap_explore",
    "devmap_literals",
    "devmap_doc_drift",
    "devmap_affected_tests",
    "devmap_suspects",
    "devmap_blast",
    "devmap_routes",
    "devmap_api_impact",
    "devmap_cypher",
]


def _yaml_server_tools(yaml_text: str, server_key: str) -> list[str]:
    in_server = False
    in_tools = False
    items: list[str] = []
    for raw in yaml_text.splitlines():
        if raw.startswith(f"  {server_key}:"):
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
    json_cfg = json.loads((config_dir / "mcp_config.json").read_text())
    yaml_text = (config_dir / "mcp_config.yaml").read_text()

    json_tools = json_cfg["mcpServers"]["waylode"]["tools"]
    yaml_tools = _yaml_server_tools(yaml_text, "waylode")

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


def test_all_21_devmap_tools_and_8_devcouncil_tools_synced_across_manifests():
    config_dir = Path(__file__).resolve().parent
    json_cfg = json.loads((config_dir / "mcp_config.json").read_text())
    yaml_text = (config_dir / "mcp_config.yaml").read_text()

    assert DEVMAP_TOOLS == EXPECTED_21_DEVMAP_TOOLS
    assert len(DEVMAP_TOOLS) == 21
    assert json_cfg["mcpServers"]["devmap"]["tools"] == EXPECTED_21_DEVMAP_TOOLS
    assert _yaml_server_tools(yaml_text, "devmap") == EXPECTED_21_DEVMAP_TOOLS

    assert len(DEVCOUNCIL_TOOLS) == 8
    assert json_cfg["mcpServers"]["devcouncil"]["tools"] == DEVCOUNCIL_TOOLS
    assert _yaml_server_tools(yaml_text, "devcouncil") == DEVCOUNCIL_TOOLS

    cfg = create_unified_antigravity_config(workspace_root=str(REPO_ROOT))
    assert cfg.get_server("devmap").enabled_tools == EXPECTED_21_DEVMAP_TOOLS
    assert cfg.get_server("devcouncil").enabled_tools == DEVCOUNCIL_TOOLS


def test_server_commands_and_env_are_valid_across_manifests():
    config_dir = Path(__file__).resolve().parent
    json_cfg = json.loads((config_dir / "mcp_config.json").read_text())
    yaml_text = (config_dir / "mcp_config.yaml").read_text()

    # JSON manifest commands: no broken `mcp serve`, `serve --mcp`, `--repo`, or DEVCOUNCIL_WORKSPACE
    dc_json = json_cfg["mcpServers"]["devcouncil"]
    assert dc_json["command"] == "devcouncil"
    assert dc_json["args"] == ["mcp"]
    assert "DEVCOUNCIL_WORKSPACE" not in json.dumps(dc_json)

    dm_json = json_cfg["mcpServers"]["devmap"]
    assert dm_json["command"] == "devmap"
    assert dm_json["args"] == ["mcp"]

    wl_json = json_cfg["mcpServers"]["waylode"]
    assert wl_json["command"] == "waylode-mcp"
    assert wl_json["args"] == []
    assert wl_json["env"].get("WAYLODE_MCP_NO_DEVMAP") == "1"

    assert "DEVCOUNCIL_WORKSPACE" not in yaml_text
    assert "--mcp" not in yaml_text
    assert "--repo" not in yaml_text
    assert "command: waylode-mcp" in yaml_text

    # Python LocalAgentConfig commands
    cfg_http = create_unified_antigravity_config(workspace_root=str(REPO_ROOT), use_http_for_devmap=True)
    dc_srv = cfg_http.get_server("devcouncil")
    assert isinstance(dc_srv, McpStdioServer)
    assert dc_srv.command == "devcouncil"
    assert dc_srv.args == ["mcp"]
    assert dc_srv.env == {"DEVCOUNCIL_PROJECT_ROOT": str(REPO_ROOT)}

    dm_srv = cfg_http.get_server("devmap")
    assert isinstance(dm_srv, McpStreamableHttpServer)
    assert dm_srv.fallback_command == "devmap"
    assert dm_srv.fallback_args == ["mcp", "--root", str(REPO_ROOT)]

    wl_srv = cfg_http.get_server("waylode")
    assert isinstance(wl_srv, McpStdioServer)
    assert wl_srv.command == "waylode-mcp"
    assert wl_srv.args == []
    assert wl_srv.env["WAYLODE_MCP_NO_DEVMAP"] == "1"

    cfg_stdio = create_unified_antigravity_config(workspace_root=str(REPO_ROOT), use_http_for_devmap=False)
    dm_stdio = cfg_stdio.get_server("devmap")
    assert isinstance(dm_stdio, McpStdioServer)
    assert dm_stdio.command == "devmap"
    assert dm_stdio.args == ["mcp", "--root", str(REPO_ROOT)]


def test_mcp_server_rejects_malformed_tool_identifiers_and_bridge_routes_a2a():
    with pytest.raises(ValueError):
        McpStdioServer(
            name="waylode",
            command="waylode-mcp",
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
    """No in-process handler, no HTTP daemon, no binary: calls must fail closed."""
    clear_in_process_handlers()
    monkeypatch.setenv("DEVMAP_DISABLE_HTTP", "1")
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: None)


@pytest.mark.parametrize(
    ("server", "tool"),
    [
        ("devcouncil", "devcouncil_verify_task"),
        ("devcouncil", "devcouncil_policy_check_write"),
        ("devcouncil", "devcouncil_checkout_task"),
        ("waylode", "waylode_collision_risk"),
        ("gitpulse", "gitpulse_collision_risk"),
        ("waylode", "waylode_worktree_create"),
        ("devmap", "devmap_status"),
    ],
)
def test_missing_server_binary_fails_closed_for_all_tools(no_server, server, tool):
    """Critical security regression: never fabricate ok/passed/allowed/clean offline."""
    res = asyncio.run(call_mcp_tool(server, tool, {"repo_path": str(REPO_ROOT)}))
    assert res["ok"] is False
    assert res["passed"] is False
    assert res["allowed"] is False
    assert res.get("status") != "clean"
    assert res.get("unavailable") is True
    assert "error" in res and res["error"]


def test_offline_fabrication_helper_is_removed():
    assert not hasattr(mcp_bridge, "_generate_offline_tool_response")


def test_bridge_source_has_no_hardcoded_user_home_paths():
    src = Path(mcp_bridge.__file__).read_text()
    assert "/Users/" not in src
    assert "/api/mcp/" not in src


def test_stdio_unknown_tool_is_retried_under_the_other_spelling(monkeypatch):
    # A server from before the rename lists gitpulse_*; one from after lists waylode_*.
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])
    called: list[str] = []

    async def fake_exec(cmd, tool_name, arguments, timeout=None):
        called.append(tool_name)
        if tool_name.startswith("waylode_"):
            return {"ok": False, "error": {"code": -32602, "message": f'unknown tool "{tool_name}"'}}
        return {"ok": True, "overlapping_files": [{"path": "src/a.rs"}]}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", fake_exec)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert called == ["waylode_collision_risk", "gitpulse_collision_risk"]
    assert res == {"ok": True, "overlapping_files": [{"path": "src/a.rs"}]}


def test_stdio_unknown_under_both_spellings_is_fail_closed_error(monkeypatch):
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])

    async def fake_exec(cmd, tool_name, arguments, timeout=None):
        return {"ok": False, "error": f'unknown tool "{tool_name}"'}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", fake_exec)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert res["passed"] is False
    assert res["allowed"] is False
    assert res["unknown_tool"] is True
    assert "waylode_collision_risk" in res["error"] and "gitpulse_collision_risk" in res["error"]


def test_server_error_and_spawn_failure_both_fail_closed(monkeypatch):
    clear_in_process_handlers()
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: ["fake-mcp"])

    async def spawn_error(cmd, tool_name, arguments, timeout=None):
        return {"ok": False, "error": "[Errno 2] No such file or directory: 'fake-mcp'"}

    monkeypatch.setattr(mcp_bridge, "_exec_mcp_stdio_jsonrpc", spawn_error)
    for server, tool in [
        ("waylode", "waylode_collision_risk"),
        ("devcouncil", "devcouncil_verify_task"),
        ("devcouncil", "devcouncil_policy_check_write"),
    ]:
        res = asyncio.run(call_mcp_tool(server, tool, {}))
        assert res["ok"] is False
        assert res["passed"] is False
        assert res["allowed"] is False
        assert res.get("status") != "clean"


def test_stdio_timeout_kills_subprocess_and_fails_closed():
    cmd = [sys.executable, "-I", "-c", "import time; time.sleep(30)"]
    res = asyncio.run(
        mcp_bridge._exec_mcp_stdio_jsonrpc(cmd, "devcouncil_verify_task", {}, timeout=0.15)
    )
    assert res["ok"] is False
    assert res["passed"] is False
    assert res["allowed"] is False
    assert res.get("timed_out") is True
    assert "timed out" in str(res["error"]).lower()


def test_timeout_budgets_defaults_and_env_overrides(monkeypatch):
    monkeypatch.delenv("ANTIGRAVITY_MCP_TIMEOUT_S", raising=False)
    monkeypatch.delenv("ANTIGRAVITY_MCP_VERIFY_TIMEOUT_S", raising=False)
    monkeypatch.delenv("DEVCOUNCIL_MCP_CALL_TIMEOUT_MS", raising=False)

    assert mcp_bridge._timeout_for_tool("devcouncil_policy_check_write") == 60.0
    assert mcp_bridge._timeout_for_tool("devcouncil_verify_task") >= 300.0

    monkeypatch.setenv("ANTIGRAVITY_MCP_TIMEOUT_S", "90")
    assert mcp_bridge._timeout_for_tool("devmap_status") == 90.0
    # Regular timeout override must not cut verify_task below its dedicated budget.
    assert mcp_bridge._timeout_for_tool("devcouncil_verify_task") >= 300.0

    monkeypatch.setenv("ANTIGRAVITY_MCP_VERIFY_TIMEOUT_S", "450")
    assert mcp_bridge._timeout_for_tool("devcouncil_verify_task") == 450.0


def test_an_mcp_tool_error_reply_is_an_error_not_ok_output():
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
    assert res["passed"] is False
    assert res["allowed"] is False
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


def test_an_ok_false_body_from_the_server_is_preserved_fail_closed(monkeypatch):
    clear_in_process_handlers()
    refusal = {"ok": False, "error": "worktree unreadable", "truncated": True, "overlapping_files": 0}
    cmd = _fake_server(refusal)
    monkeypatch.setattr(mcp_bridge, "_resolve_binary_command", lambda server_name: cmd)
    res = asyncio.run(call_mcp_tool("waylode", "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert res["allowed"] is False
    assert res["error"] == "worktree unreadable"
    assert res.get("status") != "clean"


def test_a_server_that_never_answers_tools_call_is_not_an_empty_success():
    cmd = [sys.executable, "-I", "-c", "import sys; sys.stdin.read()"]
    res = asyncio.run(mcp_bridge._exec_mcp_stdio_jsonrpc(cmd, "waylode_collision_risk", {}))
    assert res["ok"] is False
    assert res["passed"] is False
    assert res["allowed"] is False
    assert "no tools/call reply" in res["error"]


def test_devmap_http_uses_modern_jsonrpc_and_required_headers(monkeypatch):
    clear_in_process_handlers()
    captured: dict = {}

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length).decode("utf-8")
            captured["path"] = self.path
            captured["headers"] = {k.lower(): v for k, v in self.headers.items()}
            captured["body"] = json.loads(raw)
            reply = {
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "isError": False,
                    "content": [{"type": "text", "text": json.dumps({"ok": True, "store_generation": 42})}],
                },
            }
            out = json.dumps(reply).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(out)))
            self.end_headers()
            self.wfile.write(out)

        def log_message(self, format, *args):
            pass

    srv = HTTPServer(("127.0.0.1", 0), Handler)
    port = srv.server_address[1]
    thread = threading.Thread(target=srv.handle_request, daemon=True)
    thread.start()
    try:
        monkeypatch.delenv("DEVMAP_DISABLE_HTTP", raising=False)
        monkeypatch.delenv("DEVMAP_HTTP_URL", raising=False)
        monkeypatch.setenv("DEVMAP_PORT", str(port))
        res = asyncio.run(call_mcp_tool("devmap", "devmap_status", {"repo_path": str(REPO_ROOT)}))
    finally:
        thread.join(timeout=2)
        srv.server_close()

    assert res == {"ok": True, "store_generation": 42}
    assert captured["path"] == "/mcp"
    assert captured["headers"]["mcp-protocol-version"] == "2026-07-28"
    assert captured["headers"]["mcp-method"] == "tools/call"
    assert captured["headers"]["mcp-name"] == "devmap_status"
    assert captured["body"]["method"] == "tools/call"
    assert captured["body"]["params"]["name"] == "devmap_status"
    assert (
        captured["body"]["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"]
        == "2026-07-28"
    )


def test_resolve_binary_command_prefers_waylode_mcp_and_honors_env_and_local_bin(monkeypatch, tmp_path):
    monkeypatch.delenv("WAYLODE_MCP_BIN", raising=False)
    monkeypatch.delenv("GITPULSE_MCP_BIN", raising=False)
    monkeypatch.delenv("DEVCOUNCIL_BIN", raising=False)
    monkeypatch.delenv("DEVMAP_BIN", raising=False)

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

    # Env override wins over PATH, and a non-executable env override fails closed (None).
    fake_dc = tmp_path / "devcouncil"
    fake_dc.write_text("#!/bin/sh\n")
    fake_dc.chmod(0o755)
    monkeypatch.setenv("DEVCOUNCIL_BIN", str(fake_dc))
    assert mcp_bridge._resolve_binary_command("devcouncil") == [str(fake_dc), "mcp"]

    monkeypatch.setenv("DEVCOUNCIL_BIN", str(tmp_path / "missing-devcouncil"))
    assert mcp_bridge._resolve_binary_command("devcouncil") is None

    # Fallback to ~/.local/bin when not on PATH
    monkeypatch.delenv("DEVCOUNCIL_BIN", raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))
    local_bin = tmp_path / ".local" / "bin"
    local_bin.mkdir(parents=True)
    local_dm = local_bin / "devmap"
    local_dm.write_text("#!/bin/sh\n")
    local_dm.chmod(0o755)
    monkeypatch.setattr(mcp_bridge.shutil, "which", lambda name: None)
    assert mcp_bridge._resolve_binary_command("devmap") == [str(local_dm), "mcp"]
