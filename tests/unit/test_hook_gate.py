"""execution.hook_gate mode and Cursor/Grok assist install defaults."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.integrations.check import build_integration_check_report
from devcouncil.integrations.clients import hooks as hooks_client

runner = CliRunner()


def _write_config(tmp_path: Path, body: str) -> None:
    dev = tmp_path / ".devcouncil"
    dev.mkdir(parents=True, exist_ok=True)
    (dev / "config.yaml").write_text(body, encoding="utf-8")


def test_hook_gate_off_allows_no_task_shell(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    # Quote mode — bare YAML `off` parses as boolean false.
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: \"off\"\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    decision = policy.evaluate(
        {"name": "Shell", "arguments": {"command": "rm -rf src"}},
        None,
    )
    assert decision.action == "allow"


def test_hook_gate_contain_denies_no_task_shell_with_escape_hint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: contain\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    decision = policy.evaluate(
        {"name": "Shell", "arguments": {"command": "rm -rf src"}},
        None,
    )
    assert decision.action == "deny"
    assert "hook_gate.mode=off" in decision.reason
    assert "DEVCOUNCIL_HOOK_GATE" in decision.reason


def test_hook_gate_env_override_off(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: contain\n")
    monkeypatch.setenv("DEVCOUNCIL_HOOK_GATE", "off")
    policy = HookPolicy(project_root=tmp_path)
    decision = policy.evaluate(
        {"name": "Write", "arguments": {"path": "src/app.py", "content": "x = 1\n"}},
        None,
    )
    assert decision.action == "allow"


def test_hook_gate_off_still_denies_force_push(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: \"off\"\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    decision = policy.evaluate(
        {"name": "Shell", "arguments": {"command": "git push --force origin main"}},
        None,
    )
    assert decision.action == "deny"


def test_hook_gate_off_still_denies_restricted_paths(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: \"off\"\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    decision = policy.evaluate(
        {"name": "Write", "arguments": {"path": ".cursor/hooks.json", "content": "{}\n"}},
        None,
    )
    assert decision.action == "deny"


def test_cursor_assist_default_installs_post_only(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    written = hooks_client._install_cursor_hooks(tmp_path)
    assert written
    data = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    hooks = data["hooks"]
    assert "postToolUse" in hooks
    assert "preToolUse" not in hooks
    matcher = hooks["postToolUse"][0]["matcher"]
    assert "Read" not in matcher
    assert "Task" not in matcher
    assert "Shell" in matcher


def test_cursor_write_gate_installs_pre_and_reapply_strips(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_cursor_hooks(tmp_path, write_gate=True)
    data = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    assert "preToolUse" in data["hooks"]
    assert "postToolUse" in data["hooks"]

    hooks_client._install_cursor_hooks(tmp_path, write_gate=False)
    data = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    assert "preToolUse" not in data["hooks"]
    assert "postToolUse" in data["hooks"]


def test_grok_assist_default_and_write_gate(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_grok_hooks(tmp_path)
    data = json.loads((tmp_path / ".grok" / "hooks" / "devcouncil.json").read_text(encoding="utf-8"))
    assert "PostToolUse" in data["hooks"]
    assert "PreToolUse" not in data["hooks"]

    hooks_client._install_grok_hooks(tmp_path, write_gate=True)
    data = json.loads((tmp_path / ".grok" / "hooks" / "devcouncil.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in data["hooks"]


def test_check_passes_on_cursor_assist(tmp_path: Path):
    import subprocess

    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    _write_config(
        tmp_path,
        "project:\n  name: t\nintegrations:\n  cursor:\n    enabled: true\n    write_gate: false\n",
    )
    hooks_client._install_cursor_hooks(tmp_path, write_gate=False)
    report = build_integration_check_report(tmp_path)
    cursor_hooks = [r for r in report.checks if r.name == "Cursor hooks"]
    assert cursor_hooks and cursor_hooks[0].status == "ok"


def test_config_set_hook_gate_mode(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    result = runner.invoke(
        app,
        ["config", "set", "execution.hook_gate.mode", "off", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    body = (tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8")
    assert "hook_gate" in body
    assert "off" in body
