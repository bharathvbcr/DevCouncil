"""CLI wrapper tests for ``cli/commands/lease.py`` (checkout/release/list/renew).

Thin adapters over ``lease_ops`` payload functions; we monkeypatch the payload
functions and assert JSON/text rendering plus exit-code branches.
"""

from __future__ import annotations

import json

from typer.testing import CliRunner

import devcouncil.cli.commands.lease as lease_cmd
from devcouncil.cli.main import app

runner = CliRunner()


def _root(tmp_path):
    return ["--project-root", str(tmp_path)]


# --------------------------------------------------------------------------- #
# checkout                                                                     #
# --------------------------------------------------------------------------- #

def test_checkout_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "checkout_task_payload",
                        lambda *a, **k: {"ok": True, "expires_at": "later", "lease_token": "tok"})
    res = runner.invoke(app, ["checkout", "TASK-1", "--client-id", "c1", "--json", *_root(tmp_path)])
    assert res.exit_code == 0
    assert json.loads(res.stdout)["lease_token"] == "tok"


def test_checkout_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "checkout_task_payload", lambda *a, **k: {"ok": False, "error": "conflict"})
    res = runner.invoke(app, ["checkout", "TASK-1", "--client-id", "c1", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_checkout_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "checkout_task_payload",
                        lambda *a, **k: {"ok": True, "expires_at": "2099"})
    res = runner.invoke(app, ["checkout", "TASK-1", "--client-id", "c1", *_root(tmp_path)])
    assert res.exit_code == 0
    assert "Checked out TASK-1" in res.stdout and "2099" in res.stdout


def test_checkout_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "checkout_task_payload", lambda *a, **k: {"ok": False, "error": "nope"})
    res = runner.invoke(app, ["checkout", "TASK-1", "--client-id", "c1", *_root(tmp_path)])
    assert res.exit_code == 1
    assert "nope" in res.stdout


def test_checkout_passes_agent_and_force(tmp_path, monkeypatch):
    captured = {}

    def _fake(root, *, task_id, client_id, agent=None, force=False):
        captured.update({"task_id": task_id, "client_id": client_id, "agent": agent, "force": force})
        return {"ok": True, "expires_at": "x"}

    monkeypatch.setattr(lease_cmd, "checkout_task_payload", _fake)
    res = runner.invoke(app, ["checkout", "TASK-1", "--client-id", "c1", "--agent", "claude", "--force", *_root(tmp_path)])
    assert res.exit_code == 0
    assert captured == {"task_id": "TASK-1", "client_id": "c1", "agent": "claude", "force": True}


# --------------------------------------------------------------------------- #
# release                                                                      #
# --------------------------------------------------------------------------- #

def test_release_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "release_task_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, ["release", "TASK-1", "--lease-token", "t", "--json", *_root(tmp_path)])
    assert res.exit_code == 0


def test_release_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "release_task_payload", lambda *a, **k: {"ok": False, "error": "x"})
    res = runner.invoke(app, ["release", "TASK-1", "--lease-token", "t", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_release_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "release_task_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, ["release", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert "Released TASK-1" in res.stdout


def test_release_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "release_task_payload", lambda *a, **k: {"ok": False, "error": "boom"})
    res = runner.invoke(app, ["release", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert res.exit_code == 1
    assert "boom" in res.stdout


# --------------------------------------------------------------------------- #
# lease list                                                                   #
# --------------------------------------------------------------------------- #

def test_lease_list_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "list_leases_payload", lambda *a, **k: {"ok": True, "leases": []})
    res = runner.invoke(app, ["lease", "list", "--json", *_root(tmp_path)])
    assert res.exit_code == 0


def test_lease_list_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "list_leases_payload", lambda *a, **k: {"ok": False})
    res = runner.invoke(app, ["lease", "list", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_lease_list_text_rows(tmp_path, monkeypatch):
    monkeypatch.setattr(
        lease_cmd, "list_leases_payload",
        lambda *a, **k: {"ok": True, "leases": [{"task_id": "T1", "owner": "o", "expires_at": "e"}]},
    )
    res = runner.invoke(app, ["lease", "list", *_root(tmp_path)])
    assert "T1" in res.stdout and "o" in res.stdout


def test_lease_list_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "list_leases_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, ["lease", "list", *_root(tmp_path)])
    assert res.exit_code == 1
    assert "bad" in res.stdout


def test_lease_list_all_flag(tmp_path, monkeypatch):
    captured = {}

    def _fake(root, *, active_only):
        captured["active_only"] = active_only
        return {"ok": True, "leases": []}

    monkeypatch.setattr(lease_cmd, "list_leases_payload", _fake)
    res = runner.invoke(app, ["lease", "list", "--all", *_root(tmp_path)])
    assert res.exit_code == 0
    assert captured["active_only"] is False


# --------------------------------------------------------------------------- #
# lease renew                                                                  #
# --------------------------------------------------------------------------- #

def test_lease_renew_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "renew_lease_payload", lambda *a, **k: {"ok": True, "expires_at": "later"})
    res = runner.invoke(app, ["lease", "renew", "TASK-1", "--lease-token", "t", "--json", *_root(tmp_path)])
    assert res.exit_code == 0


def test_lease_renew_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "renew_lease_payload", lambda *a, **k: {"ok": False})
    res = runner.invoke(app, ["lease", "renew", "TASK-1", "--lease-token", "t", "--json", *_root(tmp_path)])
    assert res.exit_code == 1


def test_lease_renew_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "renew_lease_payload", lambda *a, **k: {"ok": True, "expires_at": "2099"})
    res = runner.invoke(app, ["lease", "renew", "TASK-1", "--lease-token", "t", "--ttl-seconds", "60", *_root(tmp_path)])
    assert "Renewed TASK-1 until 2099" in res.stdout


def test_lease_renew_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(lease_cmd, "renew_lease_payload", lambda *a, **k: {"ok": False, "error": "expired"})
    res = runner.invoke(app, ["lease", "renew", "TASK-1", "--lease-token", "t", *_root(tmp_path)])
    assert res.exit_code == 1
    assert "expired" in res.stdout
