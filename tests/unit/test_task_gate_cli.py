"""CLI wrapper tests for ``cli/commands/task_gate.py``.

These commands are thin adapters over ``task_gate_ops`` payload functions, so we
monkeypatch the payload functions and assert the CLI's JSON/text rendering and
exit-code branches (the ops themselves are covered in test_task_gate_ops.py).
"""

from __future__ import annotations

import json

from typer.testing import CliRunner

import devcouncil.cli.commands.task_gate as tg
from devcouncil.cli.main import app

runner = CliRunner()


def _root(tmp_path):
    return ["--project-root", str(tmp_path)]


# --------------------------------------------------------------------------- #
# next-task                                                                    #
# --------------------------------------------------------------------------- #

def test_next_task_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "next_task_payload", lambda *a, **k: {"ok": True, "task": {"id": "T1"}})
    res = runner.invoke(app, ["next-task", "--json", *_root(tmp_path)])
    assert res.exit_code == 0
    assert json.loads(res.stdout)["task"]["id"] == "T1"


def test_next_task_json_error_exits_1(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "next_task_payload", lambda *a, **k: {"ok": False, "error": "x"})
    res = runner.invoke(app, ["next-task", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_next_task_text_with_task(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "next_task_payload", lambda *a, **k: {"ok": True, "task": {"id": "T1", "status": "planned"}})
    res = runner.invoke(app, ["next-task", *_root(tmp_path)])
    assert res.exit_code == 0
    assert "T1" in res.stdout and "planned" in res.stdout


def test_next_task_text_no_task(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "next_task_payload", lambda *a, **k: {"ok": True, "task": None, "reason": "nothing"})
    res = runner.invoke(app, ["next-task", *_root(tmp_path)])
    assert "nothing" in res.stdout


# --------------------------------------------------------------------------- #
# scope update                                                                 #
# --------------------------------------------------------------------------- #

def test_scope_update_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "update_task_scope_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, ["scope", "update", "TASK-1", "--lease-token", "t", "--json", *_root(tmp_path)])
    assert res.exit_code == 0
    assert json.loads(res.stdout)["ok"] is True


def test_scope_update_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "update_task_scope_payload", lambda *a, **k: {"ok": False})
    res = runner.invoke(app, ["scope", "update", "TASK-1", "--lease-token", "t", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_scope_update_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "update_task_scope_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, ["scope", "update", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert res.exit_code == 0
    assert "Updated scope for TASK-1" in res.stdout


def test_scope_update_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "update_task_scope_payload", lambda *a, **k: {"ok": False, "error": "boom"})
    res = runner.invoke(app, ["scope", "update", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert res.exit_code == 1
    assert "boom" in res.stdout


# --------------------------------------------------------------------------- #
# policy-check                                                                 #
# --------------------------------------------------------------------------- #

def test_policy_check_json(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "policy_check_write_payload", lambda *a, **k: {"action": "allow", "reason": "ok"})
    res = runner.invoke(app, ["policy-check", "src/a.py", "--json", *_root(tmp_path)])
    assert res.exit_code == 0
    assert json.loads(res.stdout)["action"] == "allow"


def test_policy_check_text(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "policy_check_write_payload", lambda *a, **k: {"action": "deny", "reason": "nope"})
    res = runner.invoke(app, ["policy-check", "src/a.py", "--task-id", "T1", *_root(tmp_path)])
    assert "deny: nope" in res.stdout


# --------------------------------------------------------------------------- #
# record-command                                                              #
# --------------------------------------------------------------------------- #

def test_record_command_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "record_command_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, [
        "record-command", "TASK-1", "--lease-token", "t", "--command", "pytest", "--status", "started", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 0


def test_record_command_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "record_command_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "record-command", "TASK-1", "--lease-token", "t", "--command", "pytest", "--status", "bogus", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 1


def test_record_command_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "record_command_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, [
        "record-command", "TASK-1", "--lease-token", "t", "--command", "pytest", "--status", "finished", *_root(tmp_path),
    ])
    assert "Recorded finished for TASK-1" in res.stdout


def test_record_command_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "record_command_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "record-command", "TASK-1", "--lease-token", "t", "--command", "pytest", "--status", "started", *_root(tmp_path),
    ])
    assert res.exit_code == 1
    assert "bad" in res.stdout


# --------------------------------------------------------------------------- #
# run-cmd                                                                      #
# --------------------------------------------------------------------------- #

def test_run_cmd_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "run_command_payload", lambda *a, **k: {"ok": True, "exit_code": 0})
    res = runner.invoke(app, ["run-cmd", "TASK-1", "--lease-token", "t", "--command", "pytest", "--json", *_root(tmp_path)])
    assert res.exit_code == 0


def test_run_cmd_json_command_not_allowed_does_not_exit_1(tmp_path, monkeypatch):
    monkeypatch.setattr(
        tg, "run_command_payload",
        lambda *a, **k: {"ok": False, "code": "command_not_allowed"},
    )
    res = runner.invoke(app, ["run-cmd", "TASK-1", "--lease-token", "t", "--command", "rm", "--json", *_root(tmp_path)])
    assert res.exit_code == 0


def test_run_cmd_json_other_error_exits_1(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "run_command_payload", lambda *a, **k: {"ok": False, "code": "run_failed"})
    res = runner.invoke(app, ["run-cmd", "TASK-1", "--lease-token", "t", "--command", "x", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_run_cmd_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "run_command_payload", lambda *a, **k: {"ok": True, "exit_code": 0})
    res = runner.invoke(app, ["run-cmd", "TASK-1", "--lease-token", "t", "--command", "pytest", *_root(tmp_path)])
    assert "exit 0" in res.stdout


def test_run_cmd_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "run_command_payload", lambda *a, **k: {"ok": False, "error": "nope"})
    res = runner.invoke(app, ["run-cmd", "TASK-1", "--lease-token", "t", "--command", "x", *_root(tmp_path)])
    assert res.exit_code == 1
    assert "nope" in res.stdout


# --------------------------------------------------------------------------- #
# verify-leased                                                               #
# --------------------------------------------------------------------------- #

def test_verify_leased_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "verify_task_payload", lambda *a, **k: {"ok": True, "status": "verified"})
    res = runner.invoke(app, ["verify-leased", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert res.exit_code == 0
    assert json.loads(res.stdout)["status"] == "verified"


def test_verify_leased_blocked_exits_1(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "verify_task_payload", lambda *a, **k: {"ok": False, "status": "blocked"})
    res = runner.invoke(app, ["verify-leased", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert res.exit_code == 1


# --------------------------------------------------------------------------- #
# evidence-append / evidence-list                                             #
# --------------------------------------------------------------------------- #

def test_evidence_append_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "append_evidence_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, [
        "evidence-append", "TASK-1", "--lease-token", "t", "--command", "c", "--summary", "s", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 0


def test_evidence_append_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "append_evidence_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "evidence-append", "TASK-1", "--lease-token", "t", "--command", "c", "--summary", "s", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 1


def test_evidence_append_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "append_evidence_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, [
        "evidence-append", "TASK-1", "--lease-token", "t", "--command", "c", "--summary", "s", *_root(tmp_path),
    ])
    assert "Recorded evidence for TASK-1" in res.stdout


def test_evidence_append_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "append_evidence_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "evidence-append", "TASK-1", "--lease-token", "t", "--command", "c", "--summary", "s", *_root(tmp_path),
    ])
    assert res.exit_code == 1
    assert "bad" in res.stdout


def test_evidence_list_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "get_evidence_payload", lambda *a, **k: {"ok": True, "evidence": []})
    res = runner.invoke(app, ["evidence-list", "TASK-1", "--json", *_root(tmp_path)])
    assert res.exit_code == 0


def test_evidence_list_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "get_evidence_payload", lambda *a, **k: {"ok": False})
    res = runner.invoke(app, ["evidence-list", "TASK-1", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_evidence_list_text_rows(tmp_path, monkeypatch):
    monkeypatch.setattr(
        tg, "get_evidence_payload",
        lambda *a, **k: {"ok": True, "evidence": [{"exit_code": 0, "command": "pytest"}]},
    )
    res = runner.invoke(app, ["evidence-list", "TASK-1", *_root(tmp_path)])
    assert "pytest" in res.stdout


# --------------------------------------------------------------------------- #
# handoff-leased                                                              #
# --------------------------------------------------------------------------- #

def test_handoff_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "handoff_agent_payload", lambda *a, **k: {"ok": True, "manifest_path": "/p"})
    res = runner.invoke(app, [
        "handoff-leased", "TASK-1", "--lease-token", "t", "--from", "a", "--to", "b", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 0


def test_handoff_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "handoff_agent_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "handoff-leased", "TASK-1", "--lease-token", "t", "--from", "a", "--to", "b", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 1


def test_handoff_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "handoff_agent_payload", lambda *a, **k: {"ok": True, "manifest_path": "/tmp/m.json"})
    res = runner.invoke(app, [
        "handoff-leased", "TASK-1", "--lease-token", "t", "--from", "a", "--to", "b", *_root(tmp_path),
    ])
    assert "/tmp/m.json" in res.stdout


def test_handoff_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(tg, "handoff_agent_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "handoff-leased", "TASK-1", "--lease-token", "t", "--from", "a", "--to", "b", *_root(tmp_path),
    ])
    assert res.exit_code == 1
    assert "bad" in res.stdout
