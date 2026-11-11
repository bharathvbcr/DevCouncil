"""Unit tests for Antigravity MCP server configuration and GitPulse A2A allowlist."""

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

from antigravity.config.agent_config import (
    GITPULSE_TOOLS,
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
    "gitpulse_agent_register",
    "gitpulse_agent_peers",
    "gitpulse_agent_send",
    "gitpulse_agent_inbox",
    "gitpulse_agent_wait",
)


def _yaml_gitpulse_tools(yaml_text: str) -> list[str]:
    in_gitpulse = False
    in_tools = False
    items: list[str] = []
    for raw in yaml_text.splitlines():
        if raw.startswith("  gitpulse:"):
            in_gitpulse = True
            continue
        if in_gitpulse and raw.startswith("  ") and not raw.startswith("    "):
            break
        if in_gitpulse and raw.strip() == "tools:":
            in_tools = True
            continue
        if in_tools:
            stripped = raw.strip()
            if stripped.startswith("- "):
                items.append(stripped[2:].strip())
            elif stripped and not stripped.startswith("#"):
                break
    return items


def test_gitpulse_a2a_tools_unprefixed_and_synced_across_all_manifests():
    config_dir = Path(__file__).resolve().parent
    json_tools = json.loads((config_dir / "mcp_config.json").read_text())["mcpServers"][
        "gitpulse"
    ]["tools"]
    yaml_tools = _yaml_gitpulse_tools((config_dir / "mcp_config.yaml").read_text())

    for tool in A2A_TOOLS:
        assert validate_fastmcp_identifier(tool)
        assert not tool.startswith("mcp__")
        assert tool in GITPULSE_TOOLS

    assert json_tools == GITPULSE_TOOLS
    assert yaml_tools == GITPULSE_TOOLS
    assert len(set(GITPULSE_TOOLS)) == len(GITPULSE_TOOLS)

    cfg = create_unified_antigravity_config(workspace_root=str(REPO_ROOT))
    gp_server = cfg.get_server("gitpulse")
    assert gp_server is not None
    assert gp_server.enabled_tools == GITPULSE_TOOLS


def test_mcp_server_rejects_malformed_tool_identifiers_and_bridge_routes_a2a():
    with pytest.raises(ValueError):
        McpStdioServer(
            name="gitpulse",
            command="gitpulse-mcp",
            enabled_tools=["gitpulse:agent-send"],
        )

    seen: list[tuple[str, dict]] = []

    def handler(tool_name: str, args: dict) -> dict:
        seen.append((tool_name, args))
        return {"ok": True, "tool": tool_name}

    register_in_process_handler("gitpulse", handler)
    try:
        for tool in A2A_TOOLS:
            res = asyncio.run(call_mcp_tool("gitpulse", tool, {"repo_path": str(REPO_ROOT)}))
            assert res["ok"] is True
            assert res["tool"] == tool
        assert [name for name, _ in seen] == list(A2A_TOOLS)
    finally:
        clear_in_process_handlers()
