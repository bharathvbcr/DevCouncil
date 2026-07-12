"""CLI-level coverage for ``dev integrate`` subcommands.

Exercises the typer command layer in ``devcouncil.cli.commands.integrate`` with the
per-client client installers and ``apply_integration_target`` mocked out, so we cover
the preview/apply/validation branches without touching a real coding CLI.
"""

from __future__ import annotations

import json
from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

import devcouncil.cli.commands.integrate as integrate

runner = CliRunner()


def _ok_report():
    return SimpleNamespace(ok=True, to_json=lambda: json.dumps({"ok": True}))


def _fail_report():
    return SimpleNamespace(ok=False, to_json=lambda: json.dumps({"ok": False}))


def test_overview_lists_integration_table():
    result = runner.invoke(integrate.app, [])
    assert result.exit_code == 0
    assert "DevCouncil Coding CLI Integrations" in result.output
    assert "Codex CLI" in result.output


def test_codex_preview_uses_configure(monkeypatch, tmp_path):
    seen = {}

    def fake_configure(label, command, apply):
        seen["label"] = label
        seen["apply"] = apply
        return True

    monkeypatch.setattr(integrate, "_configure", fake_configure)
    result = runner.invoke(integrate.app, ["codex", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert seen["label"] == "Codex CLI"
    assert seen["apply"] is False


def test_codex_apply_failure_exits_1(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_configure", lambda *a, **k: False)
    result = runner.invoke(integrate.app, ["codex", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_gemini_invalid_scope_exits_2(tmp_path):
    result = runner.invoke(integrate.app, ["gemini", "--scope", "bogus", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "--scope must be" in result.output


def test_gemini_preview_ok(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_configure", lambda *a, **k: True)
    result = runner.invoke(integrate.app, ["gemini", "--scope", "user", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_claude_invalid_scope_exits_2(tmp_path):
    result = runner.invoke(integrate.app, ["claude", "--scope", "nope", "--project-root", str(tmp_path)])
    assert result.exit_code == 2


def test_claude_apply_installs_hooks_and_assets(monkeypatch, tmp_path):
    calls = {}
    monkeypatch.setattr(integrate, "_configure", lambda *a, **k: True)
    monkeypatch.setattr(integrate, "_install_claude_hooks", lambda root, write_gate: (calls.setdefault("gate", write_gate), ["h"])[1])
    monkeypatch.setattr(integrate, "_install_claude_assets", lambda root: ["a1", "a2"])
    monkeypatch.setattr(integrate, "_record_claude_config", lambda root, scope, write_gate: calls.setdefault("recorded", (scope, write_gate)))

    result = runner.invoke(integrate.app, ["claude", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Claude Code integration installed" in result.output
    assert calls["gate"] is False
    assert calls["recorded"] == ("local", False)


def test_claude_apply_write_gate_records_containment(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_configure", lambda *a, **k: True)
    monkeypatch.setattr(integrate, "_install_claude_hooks", lambda root, write_gate: ["h"])
    monkeypatch.setattr(integrate, "_install_claude_assets", lambda root: [])
    monkeypatch.setattr(integrate, "_record_claude_config", lambda *a, **k: None)

    result = runner.invoke(integrate.app, ["claude", "--apply", "--write-gate", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "with write-gate" in result.output


def test_claude_apply_asset_failure_exits_1(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_configure", lambda *a, **k: True)

    def boom(root, write_gate):
        raise OSError("disk full")

    monkeypatch.setattr(integrate, "_install_claude_hooks", boom)
    result = runner.invoke(integrate.app, ["claude", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Claude asset setup failed" in result.output


def test_claude_uninstall_reports_removed(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_uninstall_claude", lambda root: [".claude/settings.local.json"])
    result = runner.invoke(integrate.app, ["claude", "--uninstall", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Removed DevCouncil Claude integration" in result.output


def test_claude_uninstall_nothing_to_remove(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_uninstall_claude", lambda root: [])
    result = runner.invoke(integrate.app, ["claude", "--uninstall", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Nothing to remove" in result.output


def test_claude_assets_preview(monkeypatch, tmp_path):
    result = runner.invoke(integrate.app, ["claude-assets", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "preview" in result.output.lower()


def test_claude_assets_apply_writes(monkeypatch, tmp_path):
    written = [tmp_path / ".claude" / "x.md"]
    monkeypatch.setattr(integrate, "_install_claude_assets", lambda root: written)
    result = runner.invoke(integrate.app, ["claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Wrote 1 Claude asset" in result.output


def test_claude_assets_apply_failure_exits_1(monkeypatch, tmp_path):
    def boom(root):
        raise ValueError("bad")

    monkeypatch.setattr(integrate, "_install_claude_assets", boom)
    result = runner.invoke(integrate.app, ["claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_claude_plugin_preview(tmp_path):
    result = runner.invoke(integrate.app, ["claude-plugin", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "plugin" in result.output.lower()


def test_claude_plugin_apply(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_install_claude_plugin", lambda root, write_gate: ["f1", "f2", "f3"])
    result = runner.invoke(integrate.app, ["claude-plugin", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Built Claude plugin bundle" in result.output


def test_claude_plugin_apply_failure_exits_1(monkeypatch, tmp_path):
    def boom(root, write_gate):
        raise ValueError("nope")

    monkeypatch.setattr(integrate, "_install_claude_plugin", boom)
    result = runner.invoke(integrate.app, ["claude-plugin", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


@pytest.mark.parametrize("target_cmd", ["cursor", "grok", "opencode", "antigravity", "warp", "aider"])
def test_apply_target_success(monkeypatch, tmp_path, target_cmd):
    monkeypatch.setattr(integrate, "apply_integration_target", lambda root, target, **k: _ok_report())
    result = runner.invoke(integrate.app, [target_cmd, "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "configured" in result.output.lower()


@pytest.mark.parametrize("target_cmd", ["cursor", "grok", "opencode", "antigravity", "aider"])
def test_apply_target_failure_exits_1(monkeypatch, tmp_path, target_cmd):
    monkeypatch.setattr(integrate, "apply_integration_target", lambda root, target, **k: _fail_report())
    result = runner.invoke(integrate.app, [target_cmd, "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_cursor_preview_uses_configure_cursor(monkeypatch, tmp_path):
    seen = {}
    monkeypatch.setattr(integrate, "_configure_cursor", lambda root, apply: seen.setdefault("apply", apply) or True)
    result = runner.invoke(integrate.app, ["cursor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert seen["apply"] is False


def test_cli_agent_invalid_input_mode_exits_2(tmp_path):
    result = runner.invoke(
        integrate.app,
        ["cli-agent", "mybot", "--command", "mybot", "--input-mode", "bogus", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2
    assert "--input-mode must be" in result.output


def test_cli_agent_empty_command_exits_2(tmp_path):
    result = runner.invoke(
        integrate.app,
        ["cli-agent", "mybot", "--command", "   ", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2


def test_cli_agent_reserved_name_exits_2(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "is_reserved_agent_name", lambda name: True)
    result = runner.invoke(
        integrate.app,
        ["cli-agent", "codex", "--command", "codex", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2
    assert "reserved" in result.output


def test_cli_agent_unknown_profile_exits_2(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "is_reserved_agent_name", lambda name: False)
    monkeypatch.setattr(integrate, "load_agent_profiles", lambda root: {"default": {}})
    result = runner.invoke(
        integrate.app,
        ["cli-agent", "mybot", "--command", "mybot", "--default-profile", "ghost", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2
    assert "Unknown --default-profile" in result.output


def test_cli_agent_preview_shows_entry(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "is_reserved_agent_name", lambda name: False)
    monkeypatch.setattr(integrate, "load_agent_profiles", lambda root: {"default": {}})
    result = runner.invoke(
        integrate.app,
        ["cli-agent", "MyBot", "--command", "mybot", "--arg", "run", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    assert "executor preview" in result.output.lower()


def test_cli_agent_apply_writes_config(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "is_reserved_agent_name", lambda name: False)
    monkeypatch.setattr(integrate, "load_agent_profiles", lambda root: {"default": {}})
    saved = {}
    monkeypatch.setattr(integrate, "_load_raw_config", lambda root: {})
    monkeypatch.setattr(integrate, "_save_raw_config", lambda root, config: saved.update(config))
    monkeypatch.setattr(integrate, "normalize_agent_name", lambda name: name.lower())
    result = runner.invoke(
        integrate.app,
        ["cli-agent", "mybot", "--command", "mybot", "--apply", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    assert "Registered CLI executor" in result.output
    assert "mybot" in saved["integrations"]["cli_agents"]["agents"]


def test_all_invalid_gemini_scope_exits_2(tmp_path):
    result = runner.invoke(integrate.app, ["all", "--gemini-scope", "bad", "--project-root", str(tmp_path)])
    assert result.exit_code == 2


def test_all_apply_success(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "apply_integration_target", lambda root, target, **k: _ok_report())
    result = runner.invoke(integrate.app, ["all", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "configured" in result.output.lower()


def test_all_preview_configures_each_tool(monkeypatch, tmp_path):
    counter = {"configure": 0, "cursor": 0}
    monkeypatch.setattr(integrate, "_codex_command", lambda root: ["codex"])
    monkeypatch.setattr(integrate, "_gemini_command", lambda root, scope: ["gemini"])
    monkeypatch.setattr(integrate, "_claude_command", lambda root, scope: ["claude"])
    monkeypatch.setattr(integrate, "_configure", lambda *a, **k: counter.__setitem__("configure", counter["configure"] + 1))
    monkeypatch.setattr(integrate, "_configure_cursor", lambda *a, **k: counter.__setitem__("cursor", 1))
    monkeypatch.setattr(integrate, "_configure_grok", lambda *a, **k: None)
    monkeypatch.setattr(integrate, "_configure_opencode", lambda *a, **k: None)
    monkeypatch.setattr(integrate, "_configure_antigravity", lambda *a, **k: None)
    monkeypatch.setattr(integrate, "_configure_warp", lambda *a, **k: None)
    monkeypatch.setattr(integrate, "_configure_aider", lambda *a, **k: None)
    monkeypatch.setattr(integrate, "_configure_native_hooks", lambda *a, **k: None)
    result = runner.invoke(integrate.app, ["all", "--no-hooks", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert counter["configure"] == 3
    assert counter["cursor"] == 1


def test_status_runs(tmp_path):
    result = runner.invoke(integrate.app, ["status", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_matrix_runs(tmp_path):
    result = runner.invoke(integrate.app, ["matrix", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Matrix" in result.output


def test_recommend_runs(monkeypatch, tmp_path):
    monkeypatch.setattr("shutil.which", lambda *a, **k: None)
    result = runner.invoke(integrate.app, ["recommend", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_hooks_preview_no_git(tmp_path):
    result = runner.invoke(integrate.app, ["hooks", "--tool", "codex", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_hooks_apply_all_success(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "apply_integration_target", lambda root, target, **k: _ok_report())
    monkeypatch.setattr(integrate, "_install_git_map_hooks", lambda root, apply: [])
    result = runner.invoke(integrate.app, ["hooks", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Native hooks configured" in result.output


def test_hooks_check_mismatch_exits_1(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate.common, "check_hook_dev_executable", lambda root: (False, "path drift"))
    monkeypatch.setattr(integrate.common, "resolve_dev_executable", lambda root: "/usr/bin/dev")
    monkeypatch.setattr(integrate.common, "recorded_hook_dev_executable", lambda root: "/old/dev")
    result = runner.invoke(integrate.app, ["hooks", "--check", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "MISMATCH" in result.output


def test_hooks_check_ok_exits_0(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate.common, "check_hook_dev_executable", lambda root: (True, "match"))
    monkeypatch.setattr(integrate.common, "resolve_dev_executable", lambda root: "/usr/bin/dev")
    monkeypatch.setattr(integrate.common, "recorded_hook_dev_executable", lambda root: "/usr/bin/dev")
    result = runner.invoke(integrate.app, ["hooks", "--check", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "OK" in result.output


def test_uninstall_bad_target_exits_2(tmp_path):
    result = runner.invoke(integrate.app, ["uninstall", "--target", "vim", "--project-root", str(tmp_path)])
    assert result.exit_code == 2


def test_uninstall_claude_target(monkeypatch, tmp_path):
    monkeypatch.setattr(integrate, "_uninstall_claude", lambda root: ["one", "two"])
    result = runner.invoke(integrate.app, ["uninstall", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "2 change" in result.output


def test_check_delegates(monkeypatch, tmp_path):
    seen = {}
    monkeypatch.setattr(integrate, "run_integration_check", lambda *a, **k: seen.update(k))
    result = runner.invoke(integrate.app, ["check", "--strict", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert seen["strict"] is True


def test_install_git_map_hooks_writes_scripts(tmp_path, monkeypatch):
    (tmp_path / ".git" / "hooks").mkdir(parents=True)
    monkeypatch.setattr(integrate.common, "resolve_dev_executable", lambda root: "/usr/bin/dev")
    monkeypatch.setattr(integrate.common, "record_hook_dev_executable", lambda root, exe: None)
    written = integrate._install_git_map_hooks(tmp_path, apply=True)
    assert any("post-commit" in p for p in written)
    hook_file = tmp_path / ".git" / "hooks" / "post-commit"
    assert hook_file.exists()
    assert "map --if-stale" in hook_file.read_text(encoding="utf-8")


def test_install_git_map_hooks_no_git_returns_empty(tmp_path):
    assert integrate._install_git_map_hooks(tmp_path, apply=True) == []


def test_retarget_git_hook_script_replaces_command():
    existing = (
        "#!/bin/sh\n"
        "# DevCouncil: refresh repo map after git operations (best-effort).\n"
        '/old/dev map --if-stale --no-wiki --project-root "$(git rev-parse --show-toplevel)" >/dev/null 2>&1 || true\n'
    )
    updated = integrate._retarget_git_hook_script(existing, "/new/dev")
    assert "/new/dev map --if-stale" in updated
    assert "/old/dev" not in updated
