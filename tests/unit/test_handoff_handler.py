"""Coverage for the MCP handoff-agent handler (CLI subprocess mocked)."""

from __future__ import annotations

import asyncio
import json

from devcouncil.integrations.mcp.handlers import handoff


def _run(coro):
    return asyncio.run(coro)


def _call(tmp_path, arguments):
    return _run(handoff.handle_handoff_agent(tmp_path, object(), arguments))


def _text(result) -> dict:
    return json.loads(result[0].text)


def test_missing_task_id(tmp_path):
    body = _text(_call(tmp_path, {}))
    assert body["ok"] is False
    assert body["code"] == "missing_argument"
    assert body["argument"] == "task_id"


def test_missing_lease_token(tmp_path):
    body = _text(_call(tmp_path, {"task_id": "T1"}))
    assert body["argument"] == "lease_token"


def test_missing_from_agent(tmp_path):
    body = _text(_call(tmp_path, {"task_id": "T1", "lease_token": "tok"}))
    assert body["argument"] == "from_agent"


def test_missing_to_agent(tmp_path):
    body = _text(_call(tmp_path, {"task_id": "T1", "lease_token": "tok", "from_agent": "a"}))
    assert body["argument"] == "to_agent"


def test_successful_handoff(tmp_path, monkeypatch):
    captured = {}

    def fake_run_cli(args, root):
        captured["args"] = args
        captured["root"] = root
        return {"ok": True, "stdout": json.dumps({"ok": True, "task_id": "T1", "to": "b"})}

    monkeypatch.setattr(handoff, "run_cli_command", fake_run_cli)
    body = _text(
        _call(
            tmp_path,
            {
                "task_id": "T1",
                "lease_token": "tok",
                "from_agent": "a",
                "to_agent": "b",
                "instruction": "please continue",
            },
        )
    )
    assert body["ok"] is True
    assert body["to"] == "b"
    # CLI args assembled correctly
    assert captured["args"][0] == "handoff-leased"
    assert "T1" in captured["args"]
    assert "--lease-token" in captured["args"]
    assert "--instruction" in captured["args"]


def test_handoff_cli_reports_failure(tmp_path, monkeypatch):
    def fake_run_cli(args, root):
        return {"ok": True, "stdout": json.dumps({"ok": False, "error": "lease expired", "code": "lease_invalid"})}

    monkeypatch.setattr(handoff, "run_cli_command", fake_run_cli)
    body = _text(
        _call(
            tmp_path,
            {"task_id": "T1", "lease_token": "tok", "from_agent": "a", "to_agent": "b"},
        )
    )
    assert body["ok"] is False
    assert body["error"] == "lease expired"
    assert body["code"] == "lease_invalid"
    assert body["task_id"] == "T1"


def test_handoff_cli_invalid_json(tmp_path, monkeypatch):
    def fake_run_cli(args, root):
        return {"ok": False, "stdout": "not json", "stderr": "cli blew up"}

    monkeypatch.setattr(handoff, "run_cli_command", fake_run_cli)
    body = _text(
        _call(
            tmp_path,
            {"task_id": "T1", "lease_token": "tok", "from_agent": "a", "to_agent": "b"},
        )
    )
    assert body["ok"] is False
    assert body["code"] == "cli_failed"


def test_instruction_optional_omitted(tmp_path, monkeypatch):
    def fake_run_cli(args, root):
        assert "--instruction" not in args
        return {"ok": True, "stdout": json.dumps({"ok": True})}

    monkeypatch.setattr(handoff, "run_cli_command", fake_run_cli)
    body = _text(
        _call(
            tmp_path,
            {"task_id": "T1", "lease_token": "tok", "from_agent": "a", "to_agent": "b"},
        )
    )
    assert body["ok"] is True
