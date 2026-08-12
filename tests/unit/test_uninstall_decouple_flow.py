"""Plan-required uninstall/decouple flows: check integrity, CLI targets, regressions."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import yaml
from typer.testing import CliRunner

import devcouncil.cli.commands.integrate as integrate
from devcouncil.integrations.actions import (
    apply_integration_target,
    decouple_integration_target,
    uninstall_integration_target,
)
from devcouncil.integrations.check import build_integration_check_report
from devcouncil.integrations.clients import claude as claude_client
from devcouncil.integrations.clients import hooks
from devcouncil.skills.registry import LIBRARY_DIR, load_skills, scaffold_skills

runner = CliRunner()


def _init_repo(tmp_path: Path) -> None:
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True, check=True)
    (tmp_path / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\n",
        encoding="utf-8",
    )


def _write_config(root: Path, data: dict) -> None:
    path = root / ".devcouncil" / "config.yaml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(data), encoding="utf-8")


def _check_names(report, *, status: str | None = None) -> list[str]:
    rows = report.checks
    if status is not None:
        rows = [r for r in rows if r.status == status]
    return [r.name for r in rows]


def test_claude_contain_uninstall_check_ok_preserves_foreign(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    monkeypatch.setattr(claude_client.shutil, "which", lambda _cmd: None)

    hooks._install_claude_hooks(tmp_path, write_gate=True)
    settings_path = tmp_path / ".claude" / "settings.local.json"
    data = json.loads(settings_path.read_text(encoding="utf-8"))
    data["permissions"] = {"allow": ["Bash(git *)"]}
    data["hooks"]["Notification"] = [
        {"matcher": "", "hooks": [{"type": "command", "command": "echo foreign-notify"}]}
    ]
    settings_path.write_text(json.dumps(data, indent=2), encoding="utf-8")
    _write_config(
        tmp_path,
        {
            "project": {"name": "test"},
            "integrations": {"claude": {"enabled": True, "write_gate": True}},
            "execution": {"hook_gate": {"mode": "contain"}},
        },
    )

    report = uninstall_integration_target(tmp_path, "claude")
    assert report.ok

    after = json.loads(settings_path.read_text(encoding="utf-8"))
    assert after["permissions"]["allow"] == ["Bash(git *)"]
    assert "Notification" in after.get("hooks", {})
    assert "PreToolUse" not in after.get("hooks", {})
    assert "PostToolUse" not in after.get("hooks", {})

    check = build_integration_check_report(tmp_path)
    assert not any(n.endswith("hook integrity") and n.startswith("Claude") for n in _check_names(check, status="fail"))
    assert "Claude hooks" not in _check_names(check, status="fail")
    assert not any(r.name == "Claude hook integrity" for r in check.checks)


def test_cursor_contain_uninstall_check_ok_preserves_foreign_mcp(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    monkeypatch.setattr("devcouncil.cli.commands.integrate.shutil.which", lambda _cmd: None)

    apply_integration_target(tmp_path, "cursor", include_hooks=True, write_gate=True)
    mcp_path = tmp_path / ".cursor" / "mcp.json"
    mcp = json.loads(mcp_path.read_text(encoding="utf-8"))
    mcp["mcpServers"]["other"] = {"command": "other"}
    mcp_path.write_text(json.dumps(mcp, indent=2), encoding="utf-8")

    report = uninstall_integration_target(tmp_path, "cursor")
    assert report.ok

    mcp_after = json.loads(mcp_path.read_text(encoding="utf-8"))
    assert "devcouncil" not in mcp_after.get("mcpServers", {})
    assert mcp_after["mcpServers"]["other"]["command"] == "other"

    check = build_integration_check_report(tmp_path)
    assert not any(r.name == "Cursor hook integrity" for r in check.checks)
    assert "Cursor hooks" not in _check_names(check, status="fail")


def test_contain_decouple_keeps_post_and_mcp_check_ok(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    monkeypatch.setattr("devcouncil.cli.commands.integrate.shutil.which", lambda _cmd: None)

    apply_integration_target(tmp_path, "cursor", include_hooks=True, write_gate=True)
    hooks_path = tmp_path / ".cursor" / "hooks.json"
    before = json.loads(hooks_path.read_text(encoding="utf-8"))
    assert "preToolUse" in before["hooks"]
    assert "postToolUse" in before["hooks"]
    mcp_before = json.loads((tmp_path / ".cursor" / "mcp.json").read_text(encoding="utf-8"))
    assert "devcouncil" in mcp_before["mcpServers"]

    report = decouple_integration_target(tmp_path, "cursor")
    assert report.ok

    after = json.loads(hooks_path.read_text(encoding="utf-8"))
    assert "preToolUse" not in after["hooks"]
    assert "postToolUse" in after["hooks"]
    mcp_after = json.loads((tmp_path / ".cursor" / "mcp.json").read_text(encoding="utf-8"))
    assert "devcouncil" in mcp_after["mcpServers"]

    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert config["integrations"]["cursor"]["write_gate"] is False
    assert config["execution"]["hook_gate"]["mode"] == "off"

    check = build_integration_check_report(tmp_path)
    cursor_hooks = [r for r in check.checks if r.name == "Cursor hooks"]
    assert cursor_hooks and cursor_hooks[0].status == "ok"
    assert not any(r.name == "Cursor hook integrity" and r.status == "fail" for r in check.checks)


def test_opencode_decouple_keeps_after_handler_and_custom_marker(tmp_path):
    _init_repo(tmp_path)
    hooks._install_opencode_hooks(tmp_path, write_gate=True)
    plugin = tmp_path / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs"
    body = plugin.read_text(encoding="utf-8")
    assert "tool.execute.before" in body
    # Hand-edit outside the before-handler region.
    plugin.write_text(body + "\n// CUSTOM_MARKER_KEEP\n", encoding="utf-8")

    report = decouple_integration_target(tmp_path, "opencode")
    assert report.ok
    stripped = plugin.read_text(encoding="utf-8")
    assert "tool.execute.before" not in stripped
    assert "tool.execute.after" in stripped
    assert "CUSTOM_MARKER_KEEP" in stripped


def test_stop_gate_block_decouple_to_assist(tmp_path):
    _write_config(
        tmp_path,
        {
            "integrations": {"claude": {"enabled": True, "write_gate": True}},
            "execution": {
                "hook_gate": {"mode": "contain"},
                "stop_gate": {"mode": "block"},
            },
        },
    )
    (tmp_path / ".claude").mkdir()
    (tmp_path / ".claude" / "settings.local.json").write_text(
        json.dumps(
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "name": "devcouncil-pre-tool-use",
                                    "command": "dev hook pre-tool-use --client claude",
                                }
                            ],
                        }
                    ],
                    "PostToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "name": "devcouncil-post-tool-use",
                                    "command": "dev hook post-tool-use --client claude",
                                }
                            ],
                        }
                    ],
                }
            }
        ),
        encoding="utf-8",
    )
    report = decouple_integration_target(tmp_path, "claude")
    assert report.ok
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert config["execution"]["stop_gate"]["mode"] == "assist"
    assert config["execution"]["hook_gate"]["mode"] == "off"
    assert config["integrations"]["claude"]["write_gate"] is False


def test_skills_unmodified_deleted_user_edited_kept(tmp_path):
    library = [
        s for s in load_skills(LIBRARY_DIR, project_root=None, include_okf=False) if s.name == "core-engineering"
    ]
    assert library
    scaffold_skills(tmp_path, library, destinations=(".claude/skills", ".cursor/skills"))
    custom = tmp_path / ".claude" / "skills" / "my-custom-skill"
    custom.mkdir(parents=True)
    (custom / "SKILL.md").write_text("# custom\n", encoding="utf-8")

    from devcouncil.integrations.clients import common

    removed = common._remove_unmodified_library_skills(
        tmp_path, destinations=(".claude/skills", ".cursor/skills")
    )
    assert any("core-engineering" in r for r in removed)
    assert not (tmp_path / ".claude" / "skills" / "core-engineering").exists()
    assert not (tmp_path / ".cursor" / "skills" / "core-engineering").exists()
    assert custom.exists()

    scaffold_skills(tmp_path, library, destinations=(".claude/skills",))
    edited = tmp_path / ".claude" / "skills" / "core-engineering" / "SKILL.md"
    edited.write_text("# user edited\n", encoding="utf-8")
    removed2 = common._remove_unmodified_library_skills(tmp_path, destinations=(".claude/skills",))
    assert removed2 == []
    assert edited.exists()


def test_git_map_uninstall_keeps_foreign_body(tmp_path):
    hooks_dir = tmp_path / ".git" / "hooks"
    hooks_dir.mkdir(parents=True)
    path = hooks_dir / "post-commit"
    path.write_text(
        "#!/bin/sh\n"
        "echo foreign-body\n"
        "# DevCouncil: refresh repo map after git operations (best-effort).\n"
        '/usr/bin/dev map --if-stale --no-wiki --project-root "$(git rev-parse --show-toplevel)" >/dev/null 2>&1 || true\n',
        encoding="utf-8",
    )
    removed = hooks._uninstall_git_map_hooks(tmp_path)
    assert removed
    text = path.read_text(encoding="utf-8")
    assert "foreign-body" in text
    assert "DevCouncil: refresh repo map" not in text


def test_assist_leftover_pretooluse_fails_check(tmp_path):
    """Regression: assist + leftover PreToolUse + write_gate false still fails check."""
    _init_repo(tmp_path)
    hooks._install_claude_hooks(tmp_path, write_gate=True)
    _write_config(
        tmp_path,
        {
            "project": {"name": "test"},
            "integrations": {"claude": {"enabled": True, "write_gate": False}},
            "execution": {"hook_gate": {"mode": "off"}},
        },
    )
    # Leave PreToolUse installed (simulate leftover after incomplete assist).
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in settings["hooks"]

    check = build_integration_check_report(tmp_path)
    claude_hooks = [r for r in check.checks if r.name == "Claude hooks"]
    assert claude_hooks and claude_hooks[0].status == "fail"
    assert "Assist mode expected" in claude_hooks[0].details


def test_empty_orphaned_hooks_after_uninstall_not_tampered(tmp_path):
    _init_repo(tmp_path)
    cursor_dir = tmp_path / ".cursor"
    cursor_dir.mkdir()
    (cursor_dir / "hooks.json").write_text(
        json.dumps({"version": 1, "hooks": {}}),
        encoding="utf-8",
    )
    _write_config(
        tmp_path,
        {
            "project": {"name": "test"},
            "integrations": {"cursor": {"enabled": False, "write_gate": False}},
        },
    )
    check = build_integration_check_report(tmp_path)
    assert not any(r.name == "Cursor hook integrity" for r in check.checks)
    assert "Cursor hooks" not in _check_names(check, status="fail")


def test_cli_uninstall_cursor_and_opencode(monkeypatch, tmp_path):
    calls: list[str] = []

    def fake_uninstall(root, target, **kwargs):
        calls.append(target)
        from types import SimpleNamespace

        return SimpleNamespace(
            ok=True,
            results=[{"target": target, "ok": True, "message": "ok", "changes": ["x"]}],
            to_json=lambda: json.dumps({"ok": True}),
        )

    monkeypatch.setattr(integrate, "uninstall_integration_target", fake_uninstall)
    for target in ("cursor", "opencode", "all"):
        result = runner.invoke(
            integrate.app,
            ["uninstall", "--target", target, "--project-root", str(tmp_path)],
        )
        assert result.exit_code == 0, result.output
    assert calls == ["cursor", "opencode", "all"]


def test_cli_uninstall_bad_target_exits_2(tmp_path):
    result = runner.invoke(
        integrate.app,
        ["uninstall", "--target", "vim", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 2


def test_cli_all_apply_then_uninstall_all_roundtrip(monkeypatch, tmp_path):
    """Mock heavy CLI MCP probes; exercise apply all then uninstall all wiring."""
    applied: list[str] = []
    uninstalled: list[str] = []

    def fake_apply(root, target, **kwargs):
        applied.append(target)
        from types import SimpleNamespace

        return SimpleNamespace(
            ok=True,
            results=[{"target": target, "ok": True, "path": "", "message": "ok"}],
            warnings=[],
            check={"ok": True},
            to_json=lambda: json.dumps({"ok": True}),
        )

    def fake_uninstall(root, target, **kwargs):
        uninstalled.append(target)
        from types import SimpleNamespace

        return SimpleNamespace(
            ok=True,
            results=[{"target": target, "ok": True, "message": "ok", "changes": []}],
            to_json=lambda: json.dumps({"ok": True}),
        )

    monkeypatch.setattr(integrate, "apply_integration_target", fake_apply)
    monkeypatch.setattr(integrate, "uninstall_integration_target", fake_uninstall)
    monkeypatch.setattr(integrate, "run_integration_check", lambda *a, **k: None)

    apply_result = runner.invoke(
        integrate.app,
        ["all", "--apply", "--project-root", str(tmp_path)],
    )
    assert apply_result.exit_code == 0, apply_result.output
    assert "all" in applied

    uninstall_result = runner.invoke(
        integrate.app,
        ["uninstall", "--target", "all", "--project-root", str(tmp_path)],
    )
    assert uninstall_result.exit_code == 0, uninstall_result.output
    assert uninstalled == ["all"]


def test_hooks_target_uninstall_clears_enabled_check_ok(tmp_path):
    """uninstall --target hooks must clear enabled/write_gate or check stays red."""
    _init_repo(tmp_path)
    hooks._install_claude_hooks(tmp_path, write_gate=True)
    hooks._install_cursor_hooks(tmp_path, write_gate=True)
    _write_config(
        tmp_path,
        {
            "project": {"name": "test"},
            "integrations": {
                "claude": {"enabled": True, "write_gate": True},
                "cursor": {"enabled": True, "write_gate": True},
            },
            "execution": {"hook_gate": {"mode": "contain"}},
        },
    )
    report = uninstall_integration_target(tmp_path, "hooks")
    assert report.ok
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert config["integrations"]["claude"]["enabled"] is False
    assert config["integrations"]["claude"]["write_gate"] is False
    assert config["integrations"]["cursor"]["enabled"] is False
    check = build_integration_check_report(tmp_path)
    assert "Claude hooks" not in _check_names(check, status="fail")
    assert "Cursor hooks" not in _check_names(check, status="fail")
    assert not any(r.name.endswith("hook integrity") and r.status == "fail" for r in check.checks)


def test_hooks_uninstall_tool_cursor_leaves_claude(tmp_path):
    """hooks --uninstall --tool cursor must not wipe other clients' hooks."""
    from devcouncil.integrations.check import _hook_config_references_devcouncil

    _init_repo(tmp_path)
    hooks._install_claude_hooks(tmp_path, write_gate=True)
    hooks._install_cursor_hooks(tmp_path, write_gate=True)
    assert (tmp_path / ".claude" / "settings.local.json").exists()
    assert (tmp_path / ".cursor" / "hooks.json").exists()

    result = runner.invoke(
        integrate.app,
        ["hooks", "--uninstall", "--tool", "cursor", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    claude = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    assert "PostToolUse" in claude.get("hooks", {})
    cursor_path = tmp_path / ".cursor" / "hooks.json"
    if cursor_path.exists():
        assert _hook_config_references_devcouncil(cursor_path) is not True
    else:
        assert not cursor_path.exists()


def test_hooks_decouple_tool_cursor_only(tmp_path):
    _init_repo(tmp_path)
    hooks._install_claude_hooks(tmp_path, write_gate=True)
    hooks._install_cursor_hooks(tmp_path, write_gate=True)
    result = runner.invoke(
        integrate.app,
        ["hooks", "--decouple", "--tool", "cursor", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    cursor = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    assert "preToolUse" not in cursor.get("hooks", {})
    assert "postToolUse" in cursor.get("hooks", {})
    claude = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    # Claude containment left alone when --tool cursor.
    assert "PreToolUse" in claude.get("hooks", {})
