"""Tests for the complete Claude Code asset surface DevCouncil installs.

Covers the static asset generators (slash commands, subagents, output style, plugin
bundle), the settings merge (statusLine + permissions + MCP enablement), the new Claude
lifecycle hook subcommands, and the MCP prompt capability.
"""

from __future__ import annotations

import asyncio
import json
import pathlib

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.integrations import claude_assets
from devcouncil.knowledge.frontmatter import split_frontmatter

runner = CliRunner()


def _init_repo(tmp_path):
    (tmp_path / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text("models:\n  provider: anthropic\n", encoding="utf-8")


# --- static builders ------------------------------------------------------------

def test_slash_commands_have_valid_frontmatter(tmp_path):
    assets = claude_assets.build_slash_commands(tmp_path)
    names = {a.path.name for a in assets}
    assert {
        "status.md", "verify.md", "repair.md", "next.md", "plan.md", "review.md", "report.md", "map.md",
    } <= names
    for asset in assets:
        assert asset.path.parent.name == "devcouncil"  # -> /devcouncil:<name>
        meta, body = split_frontmatter(asset.content)
        assert meta.get("description")
        assert body.strip()
    verify = next(a for a in assets if a.path.name == "verify.md")
    meta, body = split_frontmatter(verify.content)
    assert "Bash(dev verify" in meta["allowed-tools"]
    assert "$ARGUMENTS" in body  # argument substitution wired through


def test_claude_bash_permission_allow_covers_slash_commands_and_hero_loop():
    rules = set(claude_assets.claude_bash_permission_allow())
    assert "Bash(dev status:*)" in rules
    assert "Bash(dev map:*)" in rules
    assert "Bash(dev go:*)" in rules
    assert "Bash(dev check:*)" in rules
    assert "Bash(dev gaps:*)" in rules
    assert "Bash(dev export:*)" in rules
    assert "Bash(dev doctor:*)" in rules
    assert "Bash(devcouncil mcp-server)" in rules


def test_subagents_declare_name_description_and_mcp_tools(tmp_path):
    assets = claude_assets.build_subagents(tmp_path)
    names = {a.path.stem for a in assets}
    assert names == {"devcouncil-implementer", "devcouncil-verifier", "devcouncil-reviewer"}
    impl = next(a for a in assets if a.path.stem == "devcouncil-implementer")
    meta, body = split_frontmatter(impl.content)
    assert meta["name"] == "devcouncil-implementer"
    assert meta["description"]
    assert "mcp__devcouncil__devcouncil_verify_task" in meta["tools"]
    assert body.strip()


def test_output_style_is_named_frontmatter(tmp_path):
    asset = claude_assets.build_output_style(tmp_path)[0]
    assert asset.path == tmp_path / ".claude" / "output-styles" / "devcouncil.md"
    meta, _ = split_frontmatter(asset.content)
    assert meta["name"] == "DevCouncil"
    assert meta["description"]


def test_plugin_bundle_is_self_contained(tmp_path):
    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.2.3", skill_assets=[])
    rels = {a.path.relative_to(tmp_path).as_posix() for a in bundle}
    base = ".devcouncil/claude-plugin"
    assert f"{base}/.claude-plugin/marketplace.json" in rels
    assert f"{base}/devcouncil/.claude-plugin/plugin.json" in rels
    assert f"{base}/devcouncil/hooks/hooks.json" in rels
    assert f"{base}/devcouncil/.mcp.json" in rels
    assert any(r.startswith(f"{base}/devcouncil/commands/devcouncil/") for r in rels)
    assert any(r.startswith(f"{base}/devcouncil/agents/") for r in rels)

    plugin = next(a for a in bundle if a.path.name == "plugin.json")
    manifest = json.loads(plugin.content)
    assert manifest["name"] == "devcouncil"
    assert manifest["version"] == "1.2.3"

    market = next(a for a in bundle if a.path.name == "marketplace.json")
    mkt = json.loads(market.content)
    assert mkt["plugins"][0]["source"] == "./devcouncil"

    mcp = next(a for a in bundle if a.path.name == ".mcp.json")
    mcp_cfg = json.loads(mcp.content)
    assert mcp_cfg["mcpServers"]["devcouncil"]["env"]["DEVCOUNCIL_PROJECT_ROOT"] == "${CLAUDE_PROJECT_DIR}"

    # Assist-mode by default: lifecycle hooks present, blocking write-gate absent.
    hooks = json.loads(next(a for a in bundle if a.path.name == "hooks.json").content)
    assert "SessionStart" in hooks["hooks"] and "UserPromptSubmit" in hooks["hooks"]
    assert "PreToolUse" not in hooks["hooks"] and "PostToolUse" not in hooks["hooks"]


def test_plugin_bundle_write_gate_includes_blocking_hooks():
    bundle = claude_assets.build_plugin_bundle(pathlib.Path("/tmp/x"), version="1.0.0", skill_assets=[], write_gate=True)
    hooks = json.loads(next(a for a in bundle if a.path.name == "hooks.json").content)
    assert "PreToolUse" in hooks["hooks"] and "PostToolUse" in hooks["hooks"]


def test_plugin_bundle_includes_lsp_for_detected_languages(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "main.go").write_text("package main\n", encoding="utf-8")

    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0", skill_assets=[])
    lsp_asset = next((a for a in bundle if a.path.name == ".lsp.json"), None)
    assert lsp_asset is not None
    assert lsp_asset.path.relative_to(tmp_path).as_posix() == ".devcouncil/claude-plugin/devcouncil/.lsp.json"

    config = json.loads(lsp_asset.content)
    # One server per detected language, in Claude Code's documented schema.
    assert set(config) == {"python", "go"}
    assert config["python"]["command"] == "pyright-langserver"
    assert config["python"]["args"] == ["--stdio"]
    assert config["python"]["extensionToLanguage"][".py"] == "python"
    assert config["go"]["command"] == "gopls"
    assert config["go"]["extensionToLanguage"][".go"] == "go"


def test_plugin_bundle_omits_lsp_when_no_supported_languages(tmp_path):
    (tmp_path / "README.md").write_text("# docs only\n", encoding="utf-8")
    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0", skill_assets=[])
    assert not any(a.path.name == ".lsp.json" for a in bundle)


def test_generated_asset_write_if_changed_is_idempotent(tmp_path):
    asset = claude_assets.build_output_style(tmp_path)[0]
    assert asset.write_if_changed() is True
    assert asset.write_if_changed() is False  # unchanged second time


# --- CLI installers -------------------------------------------------------------

def test_integrate_claude_assets_apply_writes_and_is_idempotent(tmp_path):
    _init_repo(tmp_path)
    first = runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert first.exit_code == 0, first.output
    assert (tmp_path / ".claude" / "commands" / "devcouncil" / "status.md").exists()
    assert (tmp_path / ".claude" / "agents" / "devcouncil-implementer.md").exists()
    assert (tmp_path / ".claude" / "output-styles" / "devcouncil.md").exists()

    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    assert settings["statusLine"]["command"] == "devcouncil hook claude-statusline"
    assert settings["outputStyle"] == "DevCouncil"
    assert "devcouncil" in settings["enabledMcpjsonServers"]
    assert "Bash(dev status:*)" in settings["permissions"]["allow"]
    assert "Bash(dev map:*)" in settings["permissions"]["allow"]
    assert "Bash(dev go:*)" in settings["permissions"]["allow"]

    second = runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert second.exit_code == 0
    assert "Wrote 0 Claude asset file(s)." in second.output


def test_integrate_claude_assets_preserves_existing_settings(tmp_path):
    _init_repo(tmp_path)
    claude_dir = tmp_path / ".claude"
    claude_dir.mkdir()
    (claude_dir / "settings.local.json").write_text(
        json.dumps({"permissions": {"allow": ["Bash(ls)"]}, "model": "opus"}), encoding="utf-8"
    )
    result = runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    settings = json.loads((claude_dir / "settings.local.json").read_text(encoding="utf-8"))
    assert settings["model"] == "opus"  # not clobbered
    assert "Bash(ls)" in settings["permissions"]["allow"]  # preserved
    assert "Bash(dev status:*)" in settings["permissions"]["allow"]  # merged


def test_record_claude_config_writes_integrations_section(tmp_path):
    _init_repo(tmp_path)
    from devcouncil.integrations.clients.claude import _record_claude_config

    _record_claude_config(tmp_path, scope="project", write_gate=True)
    import yaml

    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    claude = config.get("integrations", {}).get("claude", {})
    assert claude.get("enabled") is True
    assert claude.get("scope") == "project"
    assert claude.get("write_gate") is True
    assert claude.get("settings_path") == ".claude/settings.local.json"


def test_claude_config_status_detects_installed_assets(tmp_path):
    from devcouncil.integrations.check import _claude_config_status

    _init_repo(tmp_path)
    runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    status, fixable, paths = _claude_config_status(tmp_path)
    assert status == "ok"
    assert fixable is False
    assert any("settings.local.json" in path for path in paths)


def test_integrate_claude_plugin_apply_builds_bundle(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "claude-plugin", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    plugin_json = tmp_path / ".devcouncil" / "claude-plugin" / "devcouncil" / ".claude-plugin" / "plugin.json"
    market_json = tmp_path / ".devcouncil" / "claude-plugin" / ".claude-plugin" / "marketplace.json"
    assert plugin_json.exists() and market_json.exists()


def test_build_github_workflow_has_triggers_and_gated_run(tmp_path):
    asset = claude_assets.build_github_workflow(tmp_path)
    assert asset.path.relative_to(tmp_path).as_posix() == ".github/workflows/devcouncil.yml"
    content = asset.content
    # Event triggers, the read-only PR job, and the gated autonomous job.
    for expected in ("pull_request:", "workflow_dispatch:", "schedule:", "dev report", "claude -p",
                     "devcouncil_next_task", "${{ secrets.ANTHROPIC_API_KEY }}"):
        assert expected in content

    try:
        import yaml
    except ModuleNotFoundError:
        return
    doc = yaml.safe_load(content)
    assert doc["name"] == "DevCouncil"
    assert set(doc["jobs"]) == {"verify", "autonomous"}
    # PR runs are read-only (no API key); autonomous runs are guarded off PRs.
    assert doc["jobs"]["verify"]["if"] == "github.event_name == 'pull_request'"
    assert doc["jobs"]["autonomous"]["if"] == "github.event_name != 'pull_request'"


def test_integrate_claude_github_apply_writes_workflow(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "claude-github", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    workflow = tmp_path / ".github" / "workflows" / "devcouncil.yml"
    assert workflow.exists()
    assert "ANTHROPIC_API_KEY" in result.output
    # Idempotent: a second apply reports no change.
    again = runner.invoke(app, ["integrate", "claude-github", "--apply", "--project-root", str(tmp_path)])
    assert again.exit_code == 0
    assert "already up to date" in again.output


def test_integrate_claude_github_preview_does_not_write(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "claude-github", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Preview only" in result.output
    assert not (tmp_path / ".github").exists()


def test_integrate_claude_assets_preview_does_not_write(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "claude-assets", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Preview only" in result.output
    assert not (tmp_path / ".claude" / "commands").exists()


# --- new Claude lifecycle hook subcommands --------------------------------------

def test_session_start_hook_emits_additional_context(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(
        app, ["hook", "session-start", "{}", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    assert "DevCouncil" in payload["hookSpecificOutput"]["additionalContext"]


def test_user_prompt_submit_hook_emits_context(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(
        app, ["hook", "user-prompt-submit", '{"prompt":"hi"}', "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["hookSpecificOutput"]["hookEventName"] == "UserPromptSubmit"


def test_lifecycle_hooks_exit_zero_without_db(tmp_path):
    # No .devcouncil: status snapshot is None, hooks must still exit 0 and emit nothing.
    for sub in ("session-end", "pre-compact", "subagent-stop", "notification"):
        result = runner.invoke(app, ["hook", sub, "{}", "--project-root", str(tmp_path)])
        assert result.exit_code == 0, f"{sub}: {result.output}"


def test_claude_statusline_falls_back_when_uninitialized(tmp_path):
    result = runner.invoke(app, ["hook", "claude-statusline", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "not initialized" in result.stdout


def test_claude_native_hooks_assist_mode_default_has_no_write_gate(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "hooks", "--tool", "claude", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    events = set(settings["hooks"].keys())
    # Assistive lifecycle hooks present...
    assert {"Stop", "SessionStart", "UserPromptSubmit", "SessionEnd", "PreCompact", "SubagentStop", "Notification"} <= events
    # ...but the blocking write-gate is NOT installed by default.
    assert "PreToolUse" not in events and "PostToolUse" not in events


def test_claude_native_hooks_write_gate_flag_adds_blocking_gate(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(
        app, ["integrate", "hooks", "--tool", "claude", "--apply", "--write-gate", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0, result.output
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    events = set(settings["hooks"].keys())
    assert {"PreToolUse", "PostToolUse", "Stop", "SessionStart", "UserPromptSubmit"} <= events


def test_integrate_claude_uninstall_removes_everything(tmp_path):
    _init_repo(tmp_path)
    # Install assets + assistive hooks (write-gate too, to prove they're removed).
    runner.invoke(app, ["integrate", "hooks", "--tool", "claude", "--apply", "--write-gate", "--project-root", str(tmp_path)])
    runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    assert (tmp_path / ".claude" / "commands" / "devcouncil" / "status.md").exists()

    result = runner.invoke(app, ["integrate", "claude", "--uninstall", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    # Generated assets gone.
    assert not (tmp_path / ".claude" / "commands" / "devcouncil").exists()
    assert not (tmp_path / ".claude" / "agents" / "devcouncil-implementer.md").exists()
    assert not (tmp_path / ".claude" / "output-styles" / "devcouncil.md").exists()
    # Settings stripped of DevCouncil hooks/statusline/MCP enablement (file may be deleted
    # if it became empty, or retained without any devcouncil reference).
    settings_path = tmp_path / ".claude" / "settings.local.json"
    if settings_path.exists():
        text = settings_path.read_text(encoding="utf-8")
        assert "devcouncil hook" not in text
        assert "devcouncil hook claude-statusline" not in text
        settings = json.loads(text)
        assert settings.get("outputStyle") != "DevCouncil"


def test_integrate_claude_uninstall_preserves_user_settings(tmp_path):
    _init_repo(tmp_path)
    claude_dir = tmp_path / ".claude"
    claude_dir.mkdir()
    (claude_dir / "settings.local.json").write_text(
        json.dumps({"permissions": {"allow": ["Bash(ls)"]}, "model": "opus"}), encoding="utf-8"
    )
    runner.invoke(app, ["integrate", "claude-assets", "--apply", "--project-root", str(tmp_path)])
    runner.invoke(app, ["integrate", "claude", "--uninstall", "--project-root", str(tmp_path)])
    settings = json.loads((claude_dir / "settings.local.json").read_text(encoding="utf-8"))
    assert settings.get("model") == "opus"  # user key preserved
    assert "Bash(ls)" in settings["permissions"]["allow"]  # user permission preserved
    assert "Bash(dev status:*)" not in settings.get("permissions", {}).get("allow", [])  # ours removed


# --- MCP prompts ----------------------------------------------------------------

def test_mcp_server_exposes_prompts():
    from devcouncil.integrations.mcp import server

    prompts = asyncio.run(server.list_prompts())
    names = {p.name for p in prompts}
    assert "devcouncil_implement_next_task" in names
    assert "devcouncil_repair_task" in names
    result = asyncio.run(server.get_prompt("devcouncil_implement_next_task", {"client_id": "x"}))
    assert result.messages
    assert result.messages[0].role == "user"
    assert "devcouncil_checkout_task" in result.messages[0].content.text


def test_mcp_get_unknown_prompt_raises():
    from devcouncil.integrations.mcp import server

    with pytest.raises(ValueError):
        asyncio.run(server.get_prompt("does_not_exist", {}))
