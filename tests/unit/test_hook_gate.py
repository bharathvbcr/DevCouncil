"""execution.hook_gate mode and Cursor/Grok assist install defaults."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from devcouncil.app.config import HookGateConfig, load_config
from devcouncil.cli.main import app
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.integrations.check import build_integration_check_report
from devcouncil.integrations.clients import hooks as hooks_client

runner = CliRunner()


def _write_config(tmp_path: Path, body: str) -> None:
    dev = tmp_path / ".devcouncil"
    dev.mkdir(parents=True, exist_ok=True)
    (dev / "config.yaml").write_text(body, encoding="utf-8")


def test_product_default_hook_gate_is_off():
    assert HookGateConfig().mode == "off"


def test_default_config_allows_no_task_shell(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """Missing hook_gate in YAML must not fail-close interactive Shell."""
    _write_config(tmp_path, "project:\n  name: t\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    for command in ("ls", "echo hello", "huggingface-cli download foo"):
        decision = policy.evaluate({"name": "Shell", "arguments": {"command": command}}, None)
        assert decision.action == "allow", (command, decision.reason)


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


def test_hook_gate_bare_yaml_off_bool_allows_no_task_shell(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    # Bare YAML `off` becomes bool False; validator must coerce to mode off.
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: off\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    assert load_config(tmp_path).execution.hook_gate.mode == "off"
    decision = HookPolicy(project_root=tmp_path).evaluate(
        {"name": "Shell", "arguments": {"command": "ls"}},
        None,
    )
    assert decision.action == "allow"


def test_pre_tool_use_cli_allows_ls_when_hook_gate_off(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: \"off\"\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    payload = json.dumps({"tool_name": "Shell", "tool_input": {"command": "ls"}})
    result = runner.invoke(
        app,
        ["hook", "pre-tool-use", payload, "--client", "cursor", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    combined = f"{result.output}\n{getattr(result, 'stderr', '') or ''}".lower()
    assert "active task lease" not in combined


def test_pre_tool_use_cli_denies_ls_only_when_contain(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: contain\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    payload = json.dumps({"tool_name": "Shell", "tool_input": {"command": "ls"}})
    result = runner.invoke(
        app,
        ["hook", "pre-tool-use", payload, "--client", "cursor", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2
    combined = f"{result.output}\n{getattr(result, 'stderr', '') or ''}"
    assert "active task lease" in combined
    assert "hook_gate.mode=off" in combined


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
    # Assist must force runtime gate off (even if PreToolUse lingered elsewhere).
    assert load_config(tmp_path).execution.hook_gate.mode == "off"
    assert load_config(tmp_path).integrations.cursor.write_gate is False


def test_cursor_write_gate_installs_pre_and_reapply_strips(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_cursor_hooks(tmp_path, write_gate=True)
    data = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    assert "preToolUse" in data["hooks"]
    assert "postToolUse" in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "contain"
    assert load_config(tmp_path).integrations.cursor.write_gate is True

    hooks_client._install_cursor_hooks(tmp_path, write_gate=False)
    data = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    assert "preToolUse" not in data["hooks"]
    assert "postToolUse" in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "off"
    # Regression: assist + off must not deny arbitrary shell via HookPolicy.
    decision = HookPolicy(project_root=tmp_path).evaluate(
        {"name": "Shell", "arguments": {"command": "ls"}},
        None,
    )
    assert decision.action == "allow"


def test_grok_assist_default_and_write_gate(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_grok_hooks(tmp_path)
    data = json.loads((tmp_path / ".grok" / "hooks" / "devcouncil.json").read_text(encoding="utf-8"))
    assert "PostToolUse" in data["hooks"]
    assert "PreToolUse" not in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "off"

    hooks_client._install_grok_hooks(tmp_path, write_gate=True)
    data = json.loads((tmp_path / ".grok" / "hooks" / "devcouncil.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "contain"


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


def _tight_running_task():
    from devcouncil.domain.task import PlannedFile, Task

    return Task(
        id="TASK-001",
        title="T",
        description="d",
        status="running",
        allowed_commands=["pytest tests/**"],
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )


def test_hook_gate_off_allows_shell_with_active_task_tight_allowlist(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    """Leftover lease must not re-bind Shell when mode=off."""
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: \"off\"\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    task = _tight_running_task()
    for command in ("ls", "echo hello", "huggingface-cli download foo"):
        decision = policy.evaluate({"name": "Shell", "arguments": {"command": command}}, task)
        assert decision.action == "allow", (command, decision.reason)


def test_hook_gate_off_allows_write_outside_planned_with_active_task(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: \"off\"\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    decision = policy.evaluate(
        {"name": "Write", "arguments": {"path": "src/other.py", "content": "x = 1\n"}},
        _tight_running_task(),
    )
    assert decision.action == "allow"


def test_hook_gate_contain_denies_shell_and_write_with_active_task_tight_allowlist(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: contain\n")
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    policy = HookPolicy(project_root=tmp_path)
    task = _tight_running_task()
    assert (
        policy.evaluate({"name": "Shell", "arguments": {"command": "ls"}}, task).action == "deny"
    )
    assert (
        policy.evaluate(
            {"name": "Write", "arguments": {"path": "src/other.py", "content": "x = 1\n"}},
            task,
        ).action
        == "deny"
    )


def test_codex_assist_default_no_pre_tool_use(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    written = hooks_client._install_codex_hooks(tmp_path, write_gate=False)
    assert written
    data = json.loads((tmp_path / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    assert "PostToolUse" in data["hooks"]
    assert "PreToolUse" not in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "off"


def test_codex_write_gate_installs_pre_and_reapply_strips(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_codex_hooks(tmp_path, write_gate=True)
    data = json.loads((tmp_path / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "contain"

    hooks_client._install_codex_hooks(tmp_path, write_gate=False)
    data = json.loads((tmp_path / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    assert "PreToolUse" not in data["hooks"]
    assert load_config(tmp_path).execution.hook_gate.mode == "off"


def test_opencode_assist_default_no_before_hook(tmp_path: Path):
    from devcouncil.integrations.clients import opencode as opencode_client

    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_opencode_hooks(tmp_path, write_gate=False)
    body = (tmp_path / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs").read_text(
        encoding="utf-8"
    )
    assert '"tool.execute.after"' in body
    assert '"tool.execute.before"' not in body
    assert load_config(tmp_path).execution.hook_gate.mode == "off"

    # Packaged source must match assist default so copies cannot reintroduce before.
    packaged = opencode_client._opencode_plugin_source().read_text(encoding="utf-8")
    assert '"tool.execute.before"' not in packaged
    assert '"tool.execute.after"' in packaged


def test_opencode_write_gate_adds_before(tmp_path: Path):
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_opencode_hooks(tmp_path, write_gate=True)
    body = (tmp_path / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs").read_text(
        encoding="utf-8"
    )
    assert '"tool.execute.before"' in body
    assert '"tool.execute.after"' in body
    assert load_config(tmp_path).execution.hook_gate.mode == "contain"


def test_check_fails_opencode_before_under_assist(tmp_path: Path):
    import subprocess

    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    _write_config(
        tmp_path,
        "project:\n  name: t\nintegrations:\n  opencode:\n    enabled: true\n    write_gate: false\n",
    )
    hooks_client._install_opencode_hooks(tmp_path, write_gate=False)
    # Tamper: inject before under assist.
    plugin = tmp_path / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs"
    plugin.write_text(
        plugin.read_text(encoding="utf-8").replace(
            '"tool.execute.after"',
            '"tool.execute.before": async (input, output) => { runHook("pre-tool-use", {}); },\n'
            '  "tool.execute.after"',
        ),
        encoding="utf-8",
    )
    report = build_integration_check_report(tmp_path)
    oc = [r for r in report.checks if r.name == "OpenCode hook plugin"]
    assert oc and oc[0].status == "fail"


def test_check_passes_on_claude_assist(tmp_path: Path):
    import subprocess

    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    _write_config(
        tmp_path,
        "project:\n  name: t\nintegrations:\n  claude:\n    enabled: true\n    write_gate: false\n",
    )
    hooks_client._install_claude_hooks(tmp_path, write_gate=False)
    report = build_integration_check_report(tmp_path)
    claude_hooks = [r for r in report.checks if r.name == "Claude hooks"]
    assert claude_hooks and claude_hooks[0].status == "ok"


def test_check_passes_on_codex_assist(tmp_path: Path):
    import subprocess

    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    _write_config(tmp_path, "project:\n  name: t\n")
    hooks_client._install_codex_hooks(tmp_path, write_gate=False)
    report = build_integration_check_report(tmp_path)
    codex = [r for r in report.checks if r.name == "Codex hook schema"]
    assert codex and codex[0].status == "ok"
    assert "assist" in codex[0].details


def test_claude_plugin_seeds_hook_gate(tmp_path: Path):
    from devcouncil.integrations.clients import claude as claude_client

    _write_config(tmp_path, "project:\n  name: t\nexecution:\n  hook_gate:\n    mode: contain\n")
    claude_client._install_claude_plugin(tmp_path, write_gate=False)
    assert load_config(tmp_path).execution.hook_gate.mode == "off"

    claude_client._install_claude_plugin(tmp_path, write_gate=True)
    assert load_config(tmp_path).execution.hook_gate.mode == "contain"


def test_doctor_assist_write_gate_is_ok_not_risky(tmp_path: Path):
    from types import SimpleNamespace

    from devcouncil.cli.commands import doctor as doctor_cmd

    cfg = SimpleNamespace(
        execution=SimpleNamespace(
            enforce_file_scope_pre_verify=True,
            hook_gate=SimpleNamespace(mode="off"),
        ),
        integrations=SimpleNamespace(
            claude=SimpleNamespace(write_gate=False),
            cursor=SimpleNamespace(write_gate=False),
            grok=SimpleNamespace(write_gate=False),
            opencode=SimpleNamespace(write_gate=False),
        ),
    )
    rows = doctor_cmd.check_execution_containment(tmp_path, config=cfg)
    write_gate_rows = [r for r in rows if r[0].endswith("write-gate")]
    assert write_gate_rows
    assert all("[green]OK[/green]" in r[1] for r in write_gate_rows)
    assert not any("Risky" in r[1] for r in write_gate_rows)
