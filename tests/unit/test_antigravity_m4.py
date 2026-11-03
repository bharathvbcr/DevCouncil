"""
test_antigravity_m4.py - Verification suite for Milestone 4 (F27, F28, F29, F30).

Verifies:
1. Unified MCP server configurations (JSON, YAML, Python LocalAgentConfig).
2. Five lifecycle hooks (on_session_start, pre_tool_call_decide, post_tool_call, stop, on_session_end).
3. Interactive sidecar panels and sidecar.json manifests.
4. Progressive agent skills (SKILL.md format and frontmatter).
"""

import asyncio
import json
import os
import re
import sys
from pathlib import Path
import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

# Ensure local antigravity package is loaded, not stdlib xkcd easter egg
if "antigravity" in sys.modules and not hasattr(sys.modules["antigravity"], "__path__"):
    del sys.modules["antigravity"]


def parse_simple_yaml(text: str) -> dict:
    """Parses simple key-value YAML strings without external dependencies."""
    result = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if ":" in line:
            key, val = line.split(":", 1)
            key = key.strip()
            val = val.strip().strip("\"'")
            if val:
                result[key] = val
    return result

from antigravity.config.agent_config import (
    create_unified_antigravity_config,
    validate_fastmcp_identifier,
    DEVCOUNCIL_TOOLS,
    DEVMAP_TOOLS,
    GITPULSE_TOOLS,
)
from antigravity.config.mcp_bridge import (
    call_mcp_tool,
    register_in_process_handler,
    clear_in_process_handlers,
)
from antigravity.hooks.session_state import (
    HookResult,
    SessionState,
    StopArgs,
    StopDecision,
    StopHookResult,
    ToolCall,
    state,
)
from antigravity.hooks.on_session_start import on_session_start_hook
from antigravity.hooks.pre_tool_call_decide import pre_tool_call_decide_hook
from antigravity.hooks.post_tool_call import post_tool_call_hook
from antigravity.hooks.stop import stop_hook
from antigravity.hooks.on_session_end import on_session_end_hook

REPO_ROOT = Path(__file__).resolve().parent.parent.parent


# ---------------------------------------------------------------------------
# F27: Unified MCP Server Configuration Verification
# ---------------------------------------------------------------------------
def test_mcp_config_json_validity():
    config_path = REPO_ROOT / "antigravity" / "config" / "mcp_config.json"
    assert config_path.exists(), "mcp_config.json must exist"
    with open(config_path) as f:
        data = json.load(f)
    assert "mcpServers" in data
    servers = data["mcpServers"]
    assert "devcouncil" in servers
    assert "devmap" in servers
    assert "gitpulse" in servers
    # Verify FastMCP identifiers
    for s_name, s_cfg in servers.items():
        for tool in s_cfg.get("tools", []):
            assert validate_fastmcp_identifier(tool), f"Tool {tool} must satisfy FastMCP naming"


def test_mcp_config_yaml_validity():
    config_path = REPO_ROOT / "antigravity" / "config" / "mcp_config.yaml"
    assert config_path.exists(), "mcp_config.yaml must exist"
    with open(config_path) as f:
        text = f.read()
    assert "mcpServers:" in text
    assert "devcouncil:" in text
    assert "devmap:" in text
    assert "gitpulse:" in text


def test_python_local_agent_config():
    config = create_unified_antigravity_config(workspace_root=str(REPO_ROOT))
    assert len(config.mcp_servers) == 3
    tools = config.list_all_tools()
    assert "devcouncil_checkout_task" in tools
    assert "devmap_search" in tools
    assert "gitpulse_collision_risk" in tools
    assert len(config.policies) > 0


# ---------------------------------------------------------------------------
# F28: Antigravity Agent Lifecycle Hooks Verification
# ---------------------------------------------------------------------------
def test_on_session_start_hook():
    state.reset()
    state.task_id = "TASK-F28-TEST"
    state.repo_path = str(REPO_ROOT)

    res = asyncio.run(on_session_start_hook())
    assert res["ok"] is True
    assert state.lease_token is not None
    assert state.worktree_path is not None


def test_pre_tool_call_decide_clean():
    state.reset()
    state.task_id = "TASK-TEST"
    call = ToolCall(name="edit_file", arguments={"target_file": "src/safe.rs", "StartLine": 1, "EndLine": 10})
    result = asyncio.run(pre_tool_call_decide_hook(call))
    assert result.allow is True


def test_pre_tool_call_decide_secret_file_blocked():
    call = ToolCall(name="write_to_file", arguments={"target_file": ".env.production", "content": "KEY=val"})
    result = asyncio.run(pre_tool_call_decide_hook(call))
    assert result.allow is False
    assert "GateSecretScan" in result.reason


def test_pre_tool_call_decide_secret_token_in_content_blocked():
    call = ToolCall(name="write_to_file", arguments={"target_file": "src/api.rs", "CodeContent": "ghp_123456789012345678901234567890123456"})
    result = asyncio.run(pre_tool_call_decide_hook(call))
    assert result.allow is False
    assert "GateSecretScan" in result.reason


def test_pre_tool_call_decide_dangerous_command_blocked():
    call = ToolCall(name="run_command", arguments={"CommandLine": "rm -rf ."})
    result = asyncio.run(pre_tool_call_decide_hook(call))
    assert result.allow is False
    assert "COMMAND SECURITY POLICY" in result.reason


def test_pre_tool_call_decide_collision_vetoed():
    # Register in-process mock handler to simulate contention
    def mock_gitpulse(tool, args):
        if tool == "gitpulse_collision_risk":
            return {
                "ok": True,
                "status": "contended",
                "overlapping_files": [
                    {
                        "agent_id": "other_agent",
                        "worktree_path": "/tmp/wt_other",
                        "path": "src/conflict.rs",
                    }
                ],
            }
        return {"ok": True}

    register_in_process_handler("gitpulse", mock_gitpulse)
    try:
        call = ToolCall(name="edit_file", arguments={"target_file": "src/conflict.rs"})
        result = asyncio.run(pre_tool_call_decide_hook(call))
        assert result.allow is False
        assert "COLLISION HAZARD" in result.reason
    finally:
        clear_in_process_handlers()


def test_post_tool_call_hook():
    state.reset()
    state.task_id = "TASK-F28"
    call_data = {"name": "write_to_file", "arguments": {"path": "src/lib.rs"}, "error": False}
    result = asyncio.run(post_tool_call_hook(call_data))
    assert result["ok"] is True
    assert result["ledger_appended"] is True
    assert result["devmap_warmed"] is True


def test_stop_hook_passing():
    state.reset()
    state.task_id = "TASK-PASS"
    state.lease_token = "lease-token-123"

    args = StopArgs(continuation_count=0)
    result = asyncio.run(stop_hook(args))
    assert result.decision == StopDecision.ALLOW_STOP


def test_stop_hook_hero_loop_continue_on_gaps():
    state.reset()
    state.task_id = "TASK-GAPS"
    state.lease_token = "lease-token-gaps"

    def mock_devcouncil(tool, args):
        if tool == "devcouncil_verify_task":
            return {
                "ok": True,
                "passed": False,
                "blocking_gaps": [
                    {"gap_id": "GAP_COVERAGE", "severity": "BLOCKING", "message": "Diff coverage 65% < 80%"}
                ],
                "typed_next_actions": [
                    {"type": "ADD_TEST", "target": "tests/test_foo.py", "instruction": "Add test for empty input"}
                ],
            }
        return {"ok": True}

    register_in_process_handler("devcouncil", mock_devcouncil)
    try:
        args = StopArgs(continuation_count=0)
        result = asyncio.run(stop_hook(args))
        assert result.decision == StopDecision.CONTINUE
        assert "Hero Loop Attempt 1/5" in result.reason
        assert "GAP_COVERAGE" in result.reason
        assert "ADD_TEST" in result.reason
    finally:
        clear_in_process_handlers()


def test_stop_hook_threshold_bounds():
    state.reset()
    state.task_id = "TASK-GAPS"
    state.lease_token = "lease-token-gaps"
    state.continuation_threshold = 5

    args = StopArgs(continuation_count=5)
    result = asyncio.run(stop_hook(args))
    assert result.decision == StopDecision.ALLOW_STOP


def test_on_session_end_hook():
    state.reset()
    state.task_id = "TASK-END"
    state.lease_token = "lease-token-end"
    state.worktree_path = "/tmp/wt_test"

    result = asyncio.run(on_session_end_hook())
    assert result["ok"] is True
    assert result["lease_released"] is True
    assert result["worktree_pruned"] is True
    assert state.task_id is None


# ---------------------------------------------------------------------------
# F29: Interactive Sidecars Verification
# ---------------------------------------------------------------------------
def test_sidecar_manifests_and_html_files():
    sidecars_dir = REPO_ROOT / "antigravity" / "sidecars"

    # DevMap sidecar
    devmap_manifest = sidecars_dir / "devmap" / "sidecar.json"
    devmap_html = sidecars_dir / "devmap" / "map_preview.html"
    assert devmap_manifest.exists()
    assert devmap_html.exists()
    with open(devmap_manifest) as f:
        dm_json = json.load(f)
    assert dm_json["display_name"] == "DevMap Code Graph Explorer"
    with open(devmap_html) as f:
        html_content = f.read()
    assert "DevMap Code Graph Explorer" in html_content
    assert "Blast Radius" in html_content
    assert "Resolution Ladder" in html_content

    # GitPulse sidecar
    gitpulse_manifest = sidecars_dir / "gitpulse" / "sidecar.json"
    gitpulse_html = sidecars_dir / "gitpulse" / "workbench.html"
    assert gitpulse_manifest.exists()
    assert gitpulse_html.exists()
    with open(gitpulse_manifest) as f:
        gp_json = json.load(f)
    assert gp_json["display_name"] == "GitPulse Multi-Agent Workbench"
    with open(gitpulse_html) as f:
        html_content = f.read()
    assert "GitPulse Multi-Agent Workbench" in html_content
    assert "Collision Risk Heatmap" in html_content
    assert "Append-Only Ledger" in html_content

    # DevCouncil sidecar
    devcouncil_manifest = sidecars_dir / "devcouncil" / "sidecar.json"
    devcouncil_html = sidecars_dir / "devcouncil" / "hero_loop.html"
    assert devcouncil_manifest.exists()
    assert devcouncil_html.exists()
    with open(devcouncil_manifest) as f:
        dc_json = json.load(f)
    assert dc_json["display_name"] == "DevCouncil Verification & Hero Loop"
    with open(devcouncil_html) as f:
        html_content = f.read()
    assert "Hero Loop" in html_content
    assert "GateTests" in html_content
    assert "Repair Cards" in html_content

    # Top-level redirects
    assert (sidecars_dir / "map_preview.html").exists()
    assert (sidecars_dir / "workbench.html").exists()
    assert (sidecars_dir / "hero_loop.html").exists()


# ---------------------------------------------------------------------------
# F30: Progressive Agent Skills Verification
# ---------------------------------------------------------------------------
def test_progressive_agent_skills_exist():
    skills_dir = REPO_ROOT / ".agents" / "skills"
    required_skills = [
        "grill-with-docs",
        "to-spec",
        "to-tickets",
        "agent-shield",
        "scope-creep-detector",
        "systematic-debugging",
        "project-graveyard",
    ]

    for skill_name in required_skills:
        skill_file = skills_dir / skill_name / "SKILL.md"
        assert skill_file.exists(), f"Skill file {skill_file} must exist"
        with open(skill_file) as f:
            content = f.read()
        # Must have frontmatter
        assert content.startswith("---"), f"Skill {skill_name} must start with YAML frontmatter"
        parts = content.split("---", 2)
        assert len(parts) >= 3, f"Skill {skill_name} must have closing YAML frontmatter delimiter"
        frontmatter = parse_simple_yaml(parts[1])
        assert frontmatter.get("name") == skill_name
        assert "description" in frontmatter
        assert len(frontmatter["description"]) > 10
        body = parts[2]
        assert "Objective" in body or "Protocol" in body or "Rules" in body
