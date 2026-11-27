"""Unit tests for Antigravity lifecycle hooks (fail-closed pre-tool policy, stop Hero Loop schema, session hooks)."""

import asyncio
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

if "antigravity" in sys.modules and not hasattr(sys.modules["antigravity"], "__path__"):
    del sys.modules["antigravity"]

from antigravity.config.mcp_bridge import (
    clear_in_process_handlers,
    register_in_process_handler,
)
from antigravity.hooks.on_session_end import on_session_end_hook
from antigravity.hooks.on_session_start import on_session_start_hook
from antigravity.hooks.post_tool_call import post_tool_call_hook
from antigravity.hooks.pre_tool_call_decide import pre_tool_call_decide_hook
from antigravity.hooks.session_state import StopArgs, StopDecision, ToolCall, state
from antigravity.hooks.stop import stop_hook


@pytest.fixture(autouse=True)
def clean_state(tmp_path, monkeypatch):
    monkeypatch.delenv("DEVCOUNCIL_TASK_ID", raising=False)
    clear_in_process_handlers()
    state.reset()
    state.repo_path = str(tmp_path)
    state.task_id = None
    state.lease_token = None
    state.worktree_path = None
    state.active_branch = None
    state.continuation_threshold = 5
    yield
    clear_in_process_handlers()
    state.reset()


def _register_clean_waylode():
    register_in_process_handler(
        "waylode",
        lambda tool, args: {"ok": True, "overlapping_files": 0, "items": []},
    )


def test_pre_tool_unleased_omits_task_id_and_honors_policy_verdict():
    _register_clean_waylode()
    seen_payloads: list[dict] = []

    def dc_handler(tool: str, args: dict) -> dict:
        seen_payloads.append(dict(args))
        return {"ok": True, "allowed": True}

    register_in_process_handler("devcouncil", dc_handler)
    res = asyncio.run(
        pre_tool_call_decide_hook(ToolCall(name="edit_file", arguments={"target_file": "src/a.go"}))
    )
    assert res.allow is True
    assert len(seen_payloads) == 1
    assert "task_id" not in seen_payloads[0]
    assert seen_payloads[0]["path"] == "src/a.go"
    assert seen_payloads[0]["operation"] == "modify"
    assert seen_payloads[0]["repo_path"] == state.repo_path


def test_pre_tool_leased_includes_task_id_and_maps_delete_operation():
    _register_clean_waylode()
    state.task_id = "TASK-42"
    seen_payloads: list[dict] = []

    register_in_process_handler(
        "devcouncil",
        lambda tool, args: (seen_payloads.append(dict(args)), {"ok": True, "allowed": True})[1],
    )
    res = asyncio.run(
        pre_tool_call_decide_hook(ToolCall(name="delete_file", arguments={"path": "src/old.go"}))
    )
    assert res.allow is True
    assert seen_payloads[0]["task_id"] == "TASK-42"
    assert seen_payloads[0]["operation"] == "delete"


@pytest.mark.parametrize(
    "policy_reply",
    [
        {"ok": False, "code": "invalid_argument", "error": "task_id is empty"},
        {"ok": False, "code": "not_initialized", "error": "write gate unavailable"},
        {"ok": True},  # omits "allowed"
        {"ok": True, "allowed": "true"},  # non-bool "allowed"
        {"ok": True, "allowed": False, "decision": {"reason": "outside planned scope"}},
    ],
)
def test_pre_tool_fails_closed_on_policy_error_missing_allowed_or_denial(policy_reply):
    _register_clean_waylode()
    register_in_process_handler("devcouncil", lambda tool, args: policy_reply)

    res = asyncio.run(
        pre_tool_call_decide_hook(ToolCall(name="edit_file", arguments={"path": "src/a.go"}))
    )
    assert res.allow is False
    assert res.reason


def test_pre_tool_fails_closed_on_policy_exception():
    _register_clean_waylode()

    def boom(tool: str, args: dict):
        raise RuntimeError("devcouncil transport crashed")

    register_in_process_handler("devcouncil", boom)
    res = asyncio.run(
        pre_tool_call_decide_hook(ToolCall(name="edit_file", arguments={"path": "src/a.go"}))
    )
    assert res.allow is False
    assert "Fails closed" in (res.reason or "") or "unavailable" in (res.reason or "")


def test_stop_hook_parses_real_verify_mcp_result_schema():
    state.task_id = "TASK-99"
    state.lease_token = "tok-99"

    # Matches backend/go_orchestrator/devcouncil/verify/types.go:20-84 (verify.MCPResult)
    real_mcp_result = {
        "ok": True,
        "task_id": "TASK-99",
        "passed": False,
        "status": "blocked",
        "gate_mode": "enforce",
        "blocking_gaps": [
            {
                "id": "GAP-STUB-1",
                "severity": "high",
                "gap_type": "stub_detected",
                "task_id": "TASK-99",
                "description": "Found todo!() placeholder in handler",
                "recommended_fix": "Implement handler logic",
                "blocking": True,
                "file": "src/handler.rs",
                "line": 42,
            }
        ],
        "next_actions": [
            {
                "gap_id": "GAP-STUB-1",
                "gap_type": "stub_detected",
                "category": "fix_code",
                "severity": "high",
                "blocking": True,
                "action": "Replace placeholder macro with real implementation",
                "file": "src/handler.rs",
                "line": 42,
                "suggested_command": "cargo test handler",
            }
        ],
    }

    seen: list[dict] = []
    register_in_process_handler(
        "devcouncil",
        lambda tool, args: (seen.append(dict(args)), real_mcp_result)[1],
    )

    res = asyncio.run(stop_hook(StopArgs(continuation_count=0)))
    assert res.decision == StopDecision.CONTINUE
    assert seen[0]["task_id"] == "TASK-99"
    assert seen[0]["lease_token"] == "tok-99"
    assert "GAP-STUB-1" in res.reason
    assert "Found todo!() placeholder in handler" in res.reason
    assert "fix_code" in res.reason
    assert "src/handler.rs:42" in res.reason
    assert "Replace placeholder macro with real implementation" in res.reason
    assert "cargo test handler" in res.reason
    assert "GAP_UNKNOWN" not in res.reason
    assert "No description provided" not in res.reason


def test_stop_hook_fails_closed_on_bridge_error_or_degraded_status():
    state.task_id = "TASK-99"
    state.lease_token = "tok-99"

    # Bridge timeout / ok=False must NEVER allow stop
    register_in_process_handler(
        "devcouncil",
        lambda tool, args: {"ok": False, "passed": False, "error": "timed out after 360.0s"},
    )
    res_err = asyncio.run(stop_hook(StopArgs(continuation_count=0)))
    assert res_err.decision == StopDecision.CONTINUE
    assert "timed out" in res_err.reason

    # Degraded status must NEVER allow stop even if passed=True were mistakenly sent
    register_in_process_handler(
        "devcouncil",
        lambda tool, args: {
            "ok": True,
            "passed": False,
            "status": "degraded",
            "degraded_reasons": ["dcverify unavailable"],
            "blocking_gaps": [],
            "next_actions": [],
        },
    )
    res_deg = asyncio.run(stop_hook(StopArgs(continuation_count=0)))
    assert res_deg.decision == StopDecision.CONTINUE
    assert "dcverify unavailable" in res_deg.reason

    # Genuine clean pass allows stop
    register_in_process_handler(
        "devcouncil",
        lambda tool, args: {
            "ok": True,
            "passed": True,
            "status": "verified",
            "blocking_gaps": [],
            "next_actions": [],
        },
    )
    res_ok = asyncio.run(stop_hook(StopArgs(continuation_count=0)))
    assert res_ok.decision == StopDecision.ALLOW_STOP


def test_session_start_fails_closed_when_checkout_or_worktree_fails():
    state.task_id = "TASK-7"
    register_in_process_handler(
        "devcouncil",
        lambda tool, args: {"ok": False, "error": "lease held by other"},
    )
    with pytest.raises(RuntimeError, match="lease held by other"):
        asyncio.run(on_session_start_hook())

    register_in_process_handler(
        "devcouncil",
        lambda tool, args: {"ok": True, "lease_token": "tok-7"},
    )
    register_in_process_handler(
        "waylode",
        lambda tool, args: {"ok": False, "error": "worktree creation failed"},
    )
    with pytest.raises(RuntimeError, match="worktree creation failed"):
        asyncio.run(on_session_start_hook())


def test_session_end_omits_undeclared_client_id_and_reports_failure():
    state.task_id = "TASK-7"
    state.lease_token = "tok-7"
    state.worktree_path = state.repo_path + "/wt"

    dc_args: list[dict] = []
    register_in_process_handler(
        "devcouncil",
        lambda tool, args: (dc_args.append(dict(args)), {"ok": True, "released": True})[1],
    )
    register_in_process_handler(
        "waylode",
        lambda tool, args: {"ok": False, "error": "dirty worktree"},
    )
    report = asyncio.run(on_session_end_hook())
    assert "client_id" not in dc_args[0]
    assert dc_args[0]["task_id"] == "TASK-7"
    assert dc_args[0]["lease_token"] == "tok-7"
    assert report["lease_released"] is True
    assert report["worktree_pruned"] is False
    assert report["ok"] is False


def test_post_tool_call_does_not_report_ok_false_as_warmed():
    register_in_process_handler("waylode", lambda tool, args: {"ok": False, "error": "offline"})
    register_in_process_handler("devmap", lambda tool, args: {"ok": False, "error": "offline"})
    report = asyncio.run(
        post_tool_call_hook({"name": "edit_file", "arguments": {"path": "a.py"}, "error": False})
    )
    assert report["ledger_appended"] is False
    assert report["devmap_warmed"] is False
