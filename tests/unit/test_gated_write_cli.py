"""CLI wrapper tests for ``cli/commands/gated_write.py`` (write / apply-patch).

Thin adapters over ``gated_write`` payload functions; monkeypatch the payloads
and assert JSON/text rendering, stdin fallback, and exit-code branches.
"""

from __future__ import annotations

import json

from typer.testing import CliRunner

import devcouncil.cli.commands.gated_write as gw_cmd
from devcouncil.cli.main import app

runner = CliRunner()


def _root(tmp_path):
    return ["--project-root", str(tmp_path)]


# --------------------------------------------------------------------------- #
# write                                                                        #
# --------------------------------------------------------------------------- #

def test_write_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "write_file_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, [
        "write", "TASK-1", "--lease-token", "t", "--path", "src/a.py", "--content", "x=1", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 0
    assert json.loads(res.stdout)["ok"] is True


def test_write_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "write_file_payload", lambda *a, **k: {"ok": False, "error": "bad"})
    res = runner.invoke(app, [
        "write", "TASK-1", "--lease-token", "t", "--path", "src/a.py", "--content", "x", "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 1


def test_write_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "write_file_payload", lambda *a, **k: {"ok": True})
    res = runner.invoke(app, [
        "write", "TASK-1", "--lease-token", "t", "--path", "src/a.py", "--content", "x", *_root(tmp_path),
    ])
    assert "Wrote src/a.py" in res.stdout


def test_write_text_error_shows_rejected(tmp_path, monkeypatch):
    monkeypatch.setattr(
        gw_cmd, "write_file_payload",
        lambda *a, **k: {"ok": False, "rejected_files": [{"path": "src/a.py", "reason": "denied"}]},
    )
    res = runner.invoke(app, [
        "write", "TASK-1", "--lease-token", "t", "--path", "src/a.py", "--content", "x", *_root(tmp_path),
    ])
    assert res.exit_code == 1
    assert "src/a.py" in res.stdout


def test_write_reads_content_from_stdin(tmp_path, monkeypatch):
    captured = {}

    def _fake(root, *, task_id, lease_token, rel_path, content):
        captured["content"] = content
        return {"ok": True}

    monkeypatch.setattr(gw_cmd, "write_file_payload", _fake)
    res = runner.invoke(
        app,
        ["write", "TASK-1", "--lease-token", "t", "--path", "src/a.py", *_root(tmp_path)],
        input="from-stdin\n",
    )
    assert res.exit_code == 0
    assert captured["content"] == "from-stdin\n"


# --------------------------------------------------------------------------- #
# apply-patch                                                                  #
# --------------------------------------------------------------------------- #

_DIFF = "diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n@@ -1 +1 @@\n-x\n+y\n"


def test_apply_patch_json_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "apply_patch_payload", lambda *a, **k: {"ok": True, "applied_files": ["src/a.py"]})
    res = runner.invoke(app, [
        "apply-patch", "TASK-1", "--lease-token", "t", "--unified-diff", _DIFF, "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 0


def test_apply_patch_json_error(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "apply_patch_payload", lambda *a, **k: {"ok": False, "error": "x"})
    res = runner.invoke(app, [
        "apply-patch", "TASK-1", "--lease-token", "t", "--unified-diff", _DIFF, "--json", *_root(tmp_path),
    ])
    assert res.exit_code == 1


def test_apply_patch_text_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "apply_patch_payload", lambda *a, **k: {"ok": True, "applied_files": ["src/a.py"]})
    res = runner.invoke(app, [
        "apply-patch", "TASK-1", "--lease-token", "t", "--unified-diff", _DIFF, *_root(tmp_path),
    ])
    assert "Applied patch to" in res.stdout and "src/a.py" in res.stdout


def test_apply_patch_text_error(tmp_path, monkeypatch):
    monkeypatch.setattr(gw_cmd, "apply_patch_payload", lambda *a, **k: {"ok": False, "error": "boom"})
    res = runner.invoke(app, [
        "apply-patch", "TASK-1", "--lease-token", "t", "--unified-diff", _DIFF, *_root(tmp_path),
    ])
    assert res.exit_code == 1
    assert "boom" in res.stdout


def test_apply_patch_empty_diff_rejected(tmp_path, monkeypatch):
    called = {"n": 0}

    def _fake(*a, **k):
        called["n"] += 1
        return {"ok": True}

    monkeypatch.setattr(gw_cmd, "apply_patch_payload", _fake)
    res = runner.invoke(app, [
        "apply-patch", "TASK-1", "--lease-token", "t", "--unified-diff", "   ", *_root(tmp_path),
    ])
    assert res.exit_code == 1
    assert "non-empty" in res.stdout
    # The payload must not be invoked for an empty diff.
    assert called["n"] == 0


def test_apply_patch_reads_diff_from_stdin(tmp_path, monkeypatch):
    captured = {}

    def _fake(root, *, task_id, lease_token, unified_diff):
        captured["diff"] = unified_diff
        return {"ok": True, "applied_files": ["src/a.py"]}

    monkeypatch.setattr(gw_cmd, "apply_patch_payload", _fake)
    res = runner.invoke(
        app,
        ["apply-patch", "TASK-1", "--lease-token", "t", *_root(tmp_path)],
        input=_DIFF,
    )
    assert res.exit_code == 0
    assert captured["diff"] == _DIFF
