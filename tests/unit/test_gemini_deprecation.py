"""Tests for Gemini CLI deprecation."""

from __future__ import annotations

from unittest.mock import MagicMock

from devcouncil.executors.agent_registry import (
    GEMINI_DEPRECATION_MESSAGE,
    detect_available_coding_cli,
    resolve_automated_executor,
)
from devcouncil.executors.coding_cli import CodingCliExecutor
from devcouncil.integrations.actions import apply_integration_target
from devcouncil.integrations.clients.hooks import _configure_native_hooks, _upsert_hook


def _gemini_and_agy_on_path(monkeypatch):
    monkeypatch.setattr(
        "shutil.which",
        lambda c: "/usr/bin/gemini" if c == "gemini" else "/usr/bin/agy" if c == "agy" else None,
    )


def test_detect_available_coding_cli_skips_deprecated_gemini_on_path(tmp_path, monkeypatch):
    _gemini_and_agy_on_path(monkeypatch)
    assert detect_available_coding_cli(tmp_path) == "antigravity"


def test_resolve_automated_executor_skips_deprecated_gemini_on_path(tmp_path, monkeypatch):
    _gemini_and_agy_on_path(monkeypatch)
    assert resolve_automated_executor(tmp_path, None) == "antigravity"


def test_explicit_gemini_executor_still_resolves_with_warning(tmp_path, caplog):
    (tmp_path / ".devcouncil").mkdir(parents=True, exist_ok=True)
    with caplog.at_level("WARNING"):
        executor = CodingCliExecutor(tmp_path, "gemini")
    assert executor.client == "gemini"
    assert GEMINI_DEPRECATION_MESSAGE in caplog.text


def test_apply_all_skips_gemini_mcp_registration(tmp_path, monkeypatch):
    gemini_invoked = []

    def fake_run(command):
        if command and command[0] == "gemini":
            gemini_invoked.append(command)
        return 0

    monkeypatch.setattr("devcouncil.integrations.actions.shutil.which", lambda cmd: f"/usr/bin/{cmd}")
    monkeypatch.setattr("devcouncil.cli.commands.integrate._run", fake_run)
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._configure_grok",
        lambda *_args, **_kwargs: True,
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._grok_config_path",
        lambda root: root / ".grok" / "config.toml",
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._write_cursor_config",
        lambda root: root / ".cursor" / "mcp.json",
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._write_opencode_config",
        lambda root: root / "opencode.json",
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._write_antigravity_mcp_config",
        lambda root: root / ".agents" / "mcp_config.json",
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._write_warp_mcp_config",
        lambda root: root / ".devcouncil" / "integrations" / "warp-mcp.json",
    )
    for name in (
        "_record_cursor_config",
        "_record_opencode_config",
        "_record_antigravity_config",
        "_record_warp_config",
        "_record_aider_config",
    ):
        monkeypatch.setattr(f"devcouncil.cli.commands.integrate.{name}", lambda *_a, **_k: None)
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._configure_native_hooks",
        lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._install_git_map_hooks",
        lambda *_args, **_kwargs: [],
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._install_claude_assets",
        lambda *_args, **_kwargs: [],
    )

    report = apply_integration_target(tmp_path, "all", include_hooks=False)

    assert report.ok is True
    assert gemini_invoked == []


def test_upsert_hook_replaces_duplicate_commands_by_name():
    settings: dict = {
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Write|Edit",
                    "hooks": [
                        {
                            "type": "command",
                            "name": "devcouncil-pre-tool-use",
                            "command": "devcouncil hook pre-tool-use --client claude",
                            "timeout": 10000,
                        },
                        {
                            "type": "command",
                            "name": "devcouncil-pre-tool-use",
                            "command": "/repo/.venv/bin/dev hook pre-tool-use --client claude",
                            "timeout": 5000,
                        },
                    ],
                }
            ]
        }
    }
    canonical = "/repo/.venv/bin/dev hook pre-tool-use --client claude --project-root /repo"
    _upsert_hook(settings, "PreToolUse", "Write|Edit", canonical, "devcouncil-pre-tool-use")

    hooks = settings["hooks"]["PreToolUse"][0]["hooks"]
    assert len(hooks) == 1
    assert hooks[0]["command"] == canonical
    assert hooks[0]["name"] == "devcouncil-pre-tool-use"


def test_hooks_tool_all_does_not_install_gemini(tmp_path, monkeypatch):
    installed: list[str] = []

    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._install_codex_hooks",
        lambda root: installed.append("codex") or [root / ".codex" / "hooks.json"],
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._install_claude_hooks",
        lambda root, **kwargs: installed.append("claude") or [root / ".claude" / "settings.local.json"],
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._install_cursor_hooks",
        lambda root: installed.append("cursor") or [root / ".cursor" / "hooks.json"],
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._install_grok_hooks",
        lambda root: installed.append("grok") or [root / ".grok" / "hooks" / "devcouncil.json"],
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._install_opencode_hooks",
        lambda root: installed.append("opencode") or [root / "opencode.json"],
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._install_gemini_hooks",
        lambda root: (_ for _ in ()).throw(AssertionError("gemini must not be installed for tool=all")),
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks._batched_raw_config",
        lambda *_args, **_kwargs: MagicMock(__enter__=lambda s: s, __exit__=lambda *a: None),
    )
    monkeypatch.setattr(
        "devcouncil.integrations.clients.hooks.record_hook_dev_executable",
        lambda *_args, **_kwargs: None,
    )

    _configure_native_hooks(tmp_path, tool="all", apply=True)
    assert "gemini" not in installed
    assert set(installed) == {"codex", "claude", "cursor", "grok", "opencode"}
