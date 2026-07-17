"""Unit tests for the unified stop gate orchestrator."""

from __future__ import annotations

import subprocess

from devcouncil.execution.stop_gate import evaluate_stop
from devcouncil.execution.stop_gate_history import read_events
from devcouncil.execution.stop_gate_state import get_block_count, increment_block_count
from devcouncil.verification.claims.models import Kind


def _init_repo(tmp_path, yaml_body: str):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text(yaml_body, encoding="utf-8")
    return tmp_path


def test_evaluate_stop_off_mode_allows(tmp_path):
    root = _init_repo(
        tmp_path,
        "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: off\n",
    )
    result = evaluate_stop(root, {"claim_text": "All tests pass."})
    assert result.decision == "pass"
    assert not result.reason


def test_evaluate_stop_block_on_failing_test_claim(tmp_path, monkeypatch):
    root = _init_repo(
        tmp_path,
        (
            "project:\n  name: t\n"
            "commands:\n  test:\n    - python -c \"import sys; sys.exit(1)\"\n"
            "execution:\n  stop_gate:\n    mode: block\n    verify_active_task: false\n"
        ),
    )
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(root))
    result = evaluate_stop(
        root,
        {"session_id": "sess-1", "claim_text": "All tests pass."},
    )
    assert result.decision == "block"
    assert result.reason
    assert "CLAIM VERIFICATION FAILED" in result.reason
    assert get_block_count(root, "sess-1") == 1
    events = read_events(root, limit=5)
    assert events and events[0]["decision"] == "block"


def test_evaluate_stop_assist_mode_warns_not_blocks(tmp_path):
    root = _init_repo(
        tmp_path,
        (
            "project:\n  name: t\n"
            "commands:\n  test:\n    - python -c \"import sys; sys.exit(1)\"\n"
            "execution:\n  stop_gate:\n    mode: assist\n    verify_active_task: false\n"
        ),
    )
    result = evaluate_stop(
        root,
        {"session_id": "sess-2", "claim_text": "All tests pass."},
    )
    assert result.decision == "assist"
    assert not result.reason or result.system_message


def test_max_blocks_caps_to_assist(tmp_path):
    root = _init_repo(
        tmp_path,
        (
            "project:\n  name: t\n"
            "commands:\n  test:\n    - python -c \"import sys; sys.exit(1)\"\n"
            "execution:\n  stop_gate:\n    mode: block\n    max_blocks: 1\n    verify_active_task: false\n"
        ),
    )
    increment_block_count(root, "sess-3")
    result = evaluate_stop(
        root,
        {"session_id": "sess-3", "claim_text": "All tests pass."},
    )
    assert result.decision == "assist"


def test_fail_open_on_internal_error(tmp_path, monkeypatch):
    root = _init_repo(
        tmp_path,
        "project:\n  name: t\nexecution:\n  stop_gate:\n    mode: block\n",
    )

    def boom(*_a, **_k):
        raise RuntimeError("boom")

    monkeypatch.setattr(
        "devcouncil.execution.stop_gate.map_claims",
        boom,
    )
    result = evaluate_stop(root, {"claim_text": "All tests pass."})
    assert result.decision == "pass"
    assert result.fail_open


def test_stop_hook_active_fail_open_without_prior_block(tmp_path):
    root = _init_repo(
        tmp_path,
        (
            "project:\n  name: t\n"
            "commands:\n  test:\n    - python -c \"import sys; sys.exit(1)\"\n"
            "execution:\n  stop_gate:\n    mode: block\n    verify_active_task: false\n"
        ),
    )
    result = evaluate_stop(
        root,
        {
            "session_id": "sess-4",
            "stop_hook_active": True,
            "claim_text": "All tests pass.",
        },
    )
    assert result.decision == "pass"
    assert result.fail_open


def test_map_claims_file_updated_kind():
    from devcouncil.verification.claims.mapper import map_claims

    assertions = map_claims("I updated `src/foo.py` with the fix.")
    assert any(a.kind is Kind.FILE_UPDATED and a.target == "src/foo.py" for a in assertions)
