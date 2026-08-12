"""Focused tests for uninstall/decouple strip helpers (phase-1)."""
from __future__ import annotations

import json
from pathlib import Path

import yaml

from devcouncil.integrations.actions import (
    decouple_integration_target,
    uninstall_integration_target,
)
from devcouncil.integrations.clients import common, hooks
from devcouncil.integrations.clients import cursor as cursor_client
from devcouncil.skills.registry import LIBRARY_DIR, load_skills, scaffold_skills


def _write_config(root: Path, data: dict) -> None:
    path = root / ".devcouncil" / "config.yaml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(data), encoding="utf-8")


def test_strip_claude_containment_keeps_post_tool_use(tmp_path: Path):
    settings_path = tmp_path / ".claude" / "settings.local.json"
    settings_path.parent.mkdir(parents=True)
    settings_path.write_text(
        json.dumps(
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "type": "command",
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
                                    "type": "command",
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
    changed = hooks._strip_claude_containment(tmp_path)
    assert changed
    data = json.loads(settings_path.read_text(encoding="utf-8"))
    assert "PreToolUse" not in data["hooks"]
    assert "PostToolUse" in data["hooks"]


def test_decouple_forces_stop_gate_block_to_assist(tmp_path: Path):
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
                    ]
                }
            }
        ),
        encoding="utf-8",
    )
    report = decouple_integration_target(tmp_path, "claude")
    assert report.ok
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert config["integrations"]["claude"]["write_gate"] is False
    assert config["execution"]["hook_gate"]["mode"] == "off"
    assert config["execution"]["stop_gate"]["mode"] == "assist"


def test_opencode_before_handler_surgical_strip():
    contain = hooks._opencode_plugin_body(write_gate=True)
    assist = hooks._opencode_plugin_body(write_gate=False)
    stripped, ok = hooks._strip_opencode_before_handler_text(contain)
    assert ok
    assert stripped == assist
    assert "tool.execute.before" not in stripped
    assert "tool.execute.after" in stripped


def test_remove_unmodified_library_skills_keeps_edited(tmp_path: Path):
    library = [s for s in load_skills(LIBRARY_DIR, project_root=None, include_okf=False) if s.name == "core-engineering"]
    assert library
    scaffold_skills(tmp_path, library, destinations=(".claude/skills",))
    skill_dir = tmp_path / ".claude" / "skills" / "core-engineering"
    assert skill_dir.is_dir()
    # Unmodified → removed
    removed = common._remove_unmodified_library_skills(tmp_path, destinations=(".claude/skills",))
    assert any("core-engineering" in r for r in removed)
    assert not skill_dir.exists()

    scaffold_skills(tmp_path, library, destinations=(".claude/skills",))
    (skill_dir / "SKILL.md").write_text("# edited by user\n", encoding="utf-8")
    removed = common._remove_unmodified_library_skills(tmp_path, destinations=(".claude/skills",))
    assert removed == []
    assert skill_dir.exists()


def test_uninstall_git_map_hooks_preserves_foreign(tmp_path: Path):
    hooks_dir = tmp_path / ".git" / "hooks"
    hooks_dir.mkdir(parents=True)
    path = hooks_dir / "post-commit"
    path.write_text(
        "#!/bin/sh\n"
        "echo foreign\n"
        "# DevCouncil: refresh repo map after git operations (best-effort).\n"
        '/usr/bin/dev map --if-stale --no-wiki --project-root "$(git rev-parse --show-toplevel)" >/dev/null 2>&1 || true\n',
        encoding="utf-8",
    )
    removed = hooks._uninstall_git_map_hooks(tmp_path)
    assert removed
    text = path.read_text(encoding="utf-8")
    assert "foreign" in text
    assert "DevCouncil: refresh repo map" not in text
    assert "map --if-stale" not in text


def test_uninstall_cursor_removes_mcp_key_only(tmp_path: Path):
    mcp = tmp_path / ".cursor" / "mcp.json"
    mcp.parent.mkdir(parents=True)
    mcp.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "devcouncil": {"command": "devcouncil", "args": ["mcp-server"]},
                    "other": {"command": "other"},
                }
            }
        ),
        encoding="utf-8",
    )
    _write_config(tmp_path, {"integrations": {"cursor": {"enabled": True, "write_gate": True}}})
    removed = cursor_client._uninstall_cursor(tmp_path)
    assert any("mcpServers.devcouncil" in r for r in removed)
    data = json.loads(mcp.read_text(encoding="utf-8"))
    assert "devcouncil" not in data["mcpServers"]
    assert "other" in data["mcpServers"]
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert config["integrations"]["cursor"]["enabled"] is False
    assert config["integrations"]["cursor"]["write_gate"] is False


def test_codex_strip_removes_unnamed_pretooluse(tmp_path: Path):
    path = tmp_path / ".codex" / "hooks.json"
    path.parent.mkdir(parents=True)
    path.write_text(
        json.dumps(
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [{"command": "dev hook pre-tool-use --client codex"}],
                        }
                    ],
                    "PostToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "name": "devcouncil-post-tool-use",
                                    "command": "dev hook post-tool-use --client codex",
                                }
                            ],
                        }
                    ],
                }
            }
        ),
        encoding="utf-8",
    )
    changed = hooks._strip_codex_containment(tmp_path)
    assert changed
    after = json.loads(path.read_text(encoding="utf-8"))
    assert "PreToolUse" not in after["hooks"]
    assert "PostToolUse" in after["hooks"]


def test_gemini_strip_removes_unnamed_beforetool(tmp_path: Path):
    path = tmp_path / ".gemini" / "settings.json"
    path.parent.mkdir(parents=True)
    path.write_text(
        json.dumps(
            {
                "hooks": {
                    "BeforeTool": [
                        {
                            "matcher": "",
                            "hooks": [{"command": "dev hook pre-tool-use --client gemini"}],
                        }
                    ],
                    "AfterTool": [
                        {
                            "matcher": "",
                            "hooks": [
                                {
                                    "name": "devcouncil-post-tool-use",
                                    "command": "dev hook post-tool-use --client gemini",
                                }
                            ],
                        }
                    ],
                }
            }
        ),
        encoding="utf-8",
    )
    changed = hooks._strip_gemini_containment(tmp_path)
    assert changed
    after = json.loads(path.read_text(encoding="utf-8"))
    assert "BeforeTool" not in after["hooks"]
    assert "AfterTool" in after["hooks"]


def test_uninstall_opencode_removes_plugin_ref_variants(tmp_path: Path):
    from devcouncil.integrations.clients import opencode as opencode_client

    cfg = tmp_path / "opencode.json"
    plugin = tmp_path / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs"
    plugin.parent.mkdir(parents=True)
    plugin.write_text("// plugin\n", encoding="utf-8")
    cfg.write_text(
        json.dumps(
            {
                "$schema": "https://opencode.ai/config.json",
                "mcp": {"devcouncil": {"type": "local", "command": ["devcouncil"]}, "other": {}},
                "plugin": [
                    ".devcouncil/integrations/opencode_devcouncil_plugin.mjs",
                    "./other-plugin.mjs",
                ],
            }
        ),
        encoding="utf-8",
    )
    _write_config(tmp_path, {"integrations": {"opencode": {"enabled": True, "write_gate": True}}})
    removed = opencode_client._uninstall_opencode(tmp_path)
    assert any("plugin entry" in r for r in removed)
    assert any("mcp.devcouncil" in r for r in removed)
    after = json.loads(cfg.read_text(encoding="utf-8"))
    assert "devcouncil" not in after.get("mcp", {})
    assert after["mcp"]["other"] == {}
    assert after["plugin"] == ["./other-plugin.mjs"]
    assert not plugin.exists()
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert config["integrations"]["opencode"]["enabled"] is False


def test_opencode_plugin_ref_matcher_accepts_variants():
    from devcouncil.integrations.clients import opencode as opencode_client

    assert opencode_client._is_opencode_plugin_ref(
        "./.devcouncil/integrations/opencode_devcouncil_plugin.mjs"
    )
    assert opencode_client._is_opencode_plugin_ref(
        ".devcouncil/integrations/opencode_devcouncil_plugin.mjs"
    )
    assert opencode_client._is_opencode_plugin_ref(
        "/abs/.devcouncil/integrations/opencode_devcouncil_plugin.mjs"
    )
    assert not opencode_client._is_opencode_plugin_ref("./other-plugin.mjs")
    assert opencode_client._opencode_plugin_registered(
        {"plugin": [".devcouncil/integrations/opencode_devcouncil_plugin.mjs"]}
    )


def test_decouple_no_containment_client_reports_nothing(tmp_path: Path):
    report = decouple_integration_target(tmp_path, "aider")
    assert report.ok
    assert report.results[0]["changes"] == []
    assert "no containment" in report.results[0]["message"].lower()


def test_uninstall_target_all_runs(tmp_path: Path):
    report = uninstall_integration_target(tmp_path, "all")
    assert report.ok
    assert {r["target"] for r in report.results} >= {"claude", "cursor", "hooks"}
