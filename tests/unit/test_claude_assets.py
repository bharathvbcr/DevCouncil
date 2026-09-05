"""Tests for the complete Claude Code asset surface DevCouncil installs.

Covers the static asset generators (slash commands, subagents, output style, plugin
bundle), the settings merge (statusLine + permissions + MCP enablement), the new Claude
lifecycle hook subcommands, and the MCP prompt capability.
"""

from __future__ import annotations

import asyncio
import json
import os
import pathlib

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.integrations import claude_assets
from devcouncil.integrations.clients import hooks as hooks_mod
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
    assert any(r.startswith(f"{base}/devcouncil/commands/") for r in rels)
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

    # Assist-mode by default: lifecycle hooks + refresh-only PostToolUse; no PreToolUse gate.
    hooks = json.loads(next(a for a in bundle if a.path.name == "hooks.json").content)
    assert "SessionStart" in hooks["hooks"] and "UserPromptSubmit" in hooks["hooks"]
    assert "PostToolUse" in hooks["hooks"]
    assert "PreToolUse" not in hooks["hooks"]


_PLUGIN_COMMAND_NAMES = {
    "status", "next", "verify", "repair", "plan", "review", "report", "map", "wiki", "supervise",
}


def _plugin_command_paths(bundle, root: pathlib.Path) -> set[str]:
    prefix = f"{claude_assets.PLUGIN_ROOT_REL.as_posix()}/devcouncil/commands/"
    return {
        a.path.relative_to(root).as_posix()[len(prefix):]
        for a in bundle
        if a.path.relative_to(root).as_posix().startswith(prefix)
    }


def test_plugin_commands_are_flat_markdown_files(tmp_path):
    """The plugin's ``commands/`` must be FLAT ``<name>.md``, not ``commands/devcouncil/``.

    This is a *layout* assertion rather than a manifest one on purpose:
    ``claude plugin validate --strict`` passes identically in both states, because (per the
    marketplace docs) the validator does not open a plugin's command files at all. The
    behaviour it cannot see is total — with the nested layout Claude Code 2.1.259's own
    inventory reports zero commands::

        # commands/devcouncil/*.md               # commands/*.md
        $ claude --plugin-dir <bundle> plugin details devcouncil
          Skills (0)                               Skills (10)  map, next, plan, repair,
          Always-on: ~139 tok                        report, review, status, supervise,
                                                     verify, wiki
                                                   Always-on: ~322 tok

    so all ten advertised ``/devcouncil:*`` commands were dead files. The plugin *name*
    supplies the ``devcouncil:`` prefix; the directory only hides the files.
    ``test_built_bundle_command_inventory_loads`` in test_plugin_manifest_metadata.py runs
    the real loader when Claude Code is installed — this one holds the line in bare CI.
    """
    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.2.3", skill_assets=[])
    commands = _plugin_command_paths(bundle, tmp_path)

    assert commands == {f"{name}.md" for name in _PLUGIN_COMMAND_NAMES}, (
        "plugin commands/ must hold flat <name>.md files; anything with a path separator "
        f"is invisible to the plugin loader: {sorted(commands)}"
    )


def test_plugin_session_start_matcher_includes_compact(tmp_path):
    hooks = json.loads(claude_assets._plugin_hooks_json(tmp_path))
    matcher = hooks["hooks"]["SessionStart"][0]["matcher"]
    assert "compact" in matcher
    assert "clear" in matcher
    events = set(hooks["hooks"])
    assert {"PreCompact", "PostCompact", "SessionEnd"} <= events


def _plugin_hook_timeouts(hooks_json: str) -> dict[str, int]:
    """{event: timeout} for the single DevCouncil handler each plugin event carries."""
    return {
        event: group["hooks"][0]["timeout"]
        for event, groups in json.loads(hooks_json)["hooks"].items()
        for group in groups
    }


def test_plugin_hook_timeouts_are_seconds_not_milliseconds(tmp_path):
    """Claude Code's hook `timeout` field is SECONDS (docs: hooks.md, command hook fields).

    The plugin bundle once emitted 10000/150000 here, which Claude Code read as ~2.7h and
    ~41h -- a hung hook would never have been cancelled. 600s is the documented default
    for a command hook; DevCouncil's own budgets sit well under it, so a value above 600
    means the units drifted back to milliseconds.
    """
    _init_repo(tmp_path)
    timeouts = _plugin_hook_timeouts(claude_assets._plugin_hooks_json(tmp_path, write_gate=True))

    assert timeouts, "plugin bundle emitted no hooks"
    assert all(0 < value <= 600 for value in timeouts.values()), timeouts
    # Default lifecycle/tool hooks get the shared 10s budget.
    assert timeouts["PostToolUse"] == hooks_mod.DEFAULT_HOOK_TIMEOUT_SECONDS == 10
    assert timeouts["PreToolUse"] == 10
    assert timeouts["Notification"] == 10


def test_plugin_stop_hooks_use_stop_gate_timeout_in_seconds(tmp_path):
    """Stop/SubagentStop run claims + verification, so they get the longer budget -- 150
    seconds, not the 150000 the bundle used to declare."""
    _init_repo(tmp_path)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: t\nexecution:\n  stop_gate:\n"
        "    mode: assist\n    check_claims: true\n    verify_active_task: true\n",
        encoding="utf-8",
    )
    timeouts = _plugin_hook_timeouts(claude_assets._plugin_hooks_json(tmp_path))

    assert hooks_mod._stop_hook_timeout_seconds(tmp_path) == 150
    assert timeouts["Stop"] == 150
    assert timeouts["SubagentStop"] == 150
    # A non-stop-gate hook must not inherit the long budget.
    assert timeouts["PostToolUse"] == 10


@pytest.mark.parametrize("write_gate", [False, True])
def test_plugin_bundle_and_settings_hooks_cannot_drift(tmp_path, write_gate):
    """The plugin bundle and .claude/settings.local.json describe the same hooks.

    Both generators walk the shared ``CLAUDE_HOOK_SPECS`` table; this pins the two
    outputs together so a future edit to one has to move the table, not just that file.
    Commands legitimately differ (absolute `dev` path vs ${CLAUDE_PROJECT_DIR}), so this
    compares the event/matcher/timeout triples that must agree.
    """
    _init_repo(tmp_path)
    hooks_mod._install_claude_hooks(tmp_path, write_gate=write_gate)
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))

    def triples(config: dict) -> set[tuple[str, str, int]]:
        return {
            (event, group.get("matcher", ""), hook["timeout"])
            for event, groups in config["hooks"].items()
            for group in groups
            for hook in group["hooks"]
            if hook.get("name", "").startswith("devcouncil-") or "devcouncil hook " in hook["command"]
        }

    def entry_count(config: dict) -> int:
        return sum(len(group["hooks"]) for groups in config["hooks"].values() for group in groups)

    plugin = json.loads(claude_assets._plugin_hooks_json(tmp_path, write_gate=write_gate))
    assert triples(plugin) == triples(settings)
    # Sets alone would tolerate a duplicated event, so pin the handler count too.
    assert entry_count(plugin) == entry_count(settings) == len(
        hooks_mod.claude_hook_specs(write_gate=write_gate, project_root=tmp_path)
    )
    assert ("PreToolUse" in plugin["hooks"]) is write_gate


def _fake_venv(root: pathlib.Path) -> pathlib.Path:
    """A project venv holding executable `dev` and `devcouncil` binaries."""
    venv_bin = root / ".venv" / ("Scripts" if os.name == "nt" else "bin")
    venv_bin.mkdir(parents=True, exist_ok=True)
    for name in ("dev", "devcouncil"):
        binary = venv_bin / name
        binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        binary.chmod(0o755)
    return venv_bin


def test_plugin_mcp_command_is_an_absolute_binary_with_path_injected(tmp_path):
    """The plugin's MCP server must not be a bare PATH lookup.

    The bundle shipped `"command": "devcouncil"`, but `/plugin install` installs no Python
    package and the plugin carried no `bin/` and no `PATH` — so on any machine whose Claude
    Code process lacked the project venv on PATH the MCP server simply never started, and
    every DevCouncil tool with it. The repo already solved exactly this for Cursor; the
    plugin now shares that resolver (`common.resolve_devcouncil_executable` /
    `common.venv_augmented_path`) instead of re-deciding.
    """
    venv_bin = _fake_venv(tmp_path)

    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0", skill_assets=[])
    server = json.loads(next(a for a in bundle if a.path.name == ".mcp.json").content)["mcpServers"]["devcouncil"]

    command = pathlib.Path(server["command"])
    assert command.is_absolute(), f"MCP command is a bare PATH lookup: {server['command']!r}"
    assert command.is_file(), f"MCP command does not exist: {command}"
    assert command == venv_bin / command.name, "MCP command should resolve to the project venv"
    assert server["env"]["PATH"].split(os.pathsep)[0] == str(venv_bin), (
        f"project venv is not first on the MCP server's PATH: {server['env']['PATH']!r}"
    )
    assert server["env"]["DEVCOUNCIL_PROJECT_ROOT"] == "${CLAUDE_PROJECT_DIR}"


def test_plugin_hook_commands_invoke_an_absolute_binary(tmp_path):
    """All bundled hooks must invoke a real binary, not a bare `devcouncil`.

    A hook subprocess inherits the host's PATH, which on a clean machine has no DevCouncil
    on it; every one of the bundled hooks failed there. `.claude/settings.local.json` already
    got this right via `_hook_command`, so the plugin was the one surface still emitting a
    bare name.
    """
    venv_bin = _fake_venv(tmp_path)
    devcouncil = venv_bin / "devcouncil"

    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0", skill_assets=[], write_gate=True)
    hooks = json.loads(next(a for a in bundle if a.path.name == "hooks.json").content)["hooks"]

    commands = [entry["command"] for groups in hooks.values() for group in groups for entry in group["hooks"]]
    assert len(commands) == len(hooks_mod.claude_hook_specs(write_gate=True, project_root=tmp_path))
    for command in commands:
        assert command.startswith(f"{devcouncil} "), (
            f"hook command does not start with the resolved binary {devcouncil}: {command!r}"
        )
        # ${CLAUDE_PROJECT_DIR} still resolves the repo at runtime.
        assert '--project-root "${CLAUDE_PROJECT_DIR}"' in command


def test_plugin_ships_executable_bin_launchers(tmp_path):
    """`bin/` puts `dev`/`devcouncil` on the Bash tool's PATH while the plugin is enabled.

    The bundled slash commands shell out to bare `dev status` etc.; without this the ten
    commands restored by the flat-`commands/` fix would load and then fail on a machine
    with no DevCouncil on PATH. `bin/` is the plugin spec's own slot for this ("Executables
    added to the Bash tool's PATH and invokable as bare commands while the plugin is
    enabled"), so the command markdown keeps its portable bare name and its matching
    `allowed-tools` prefix.
    """
    venv_bin = _fake_venv(tmp_path)

    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0", skill_assets=[])
    launchers = {
        a.path.name: a for a in bundle
        if a.path.parent.name == "bin" and a.path.parent.parent.name == "devcouncil"
    }
    assert set(launchers) == {"dev", "devcouncil"}, sorted(launchers)

    for name, asset in launchers.items():
        assert asset.executable, f"bin/{name} would be written without its +x bit"
        assert f"exec {venv_bin / name} " in asset.content, asset.content
        assert asset.write_if_changed() is True
        assert os.access(asset.path, os.X_OK), f"bin/{name} is not executable on disk"
        # A stripped +x bit must count as changed, or a re-run would no-op over a dead shim.
        asset.path.chmod(0o644)
        assert asset.write_if_changed() is True
        assert asset.write_if_changed() is False


def test_plugin_bundle_write_gate_includes_blocking_hooks():
    bundle = claude_assets.build_plugin_bundle(pathlib.Path("/tmp/x"), version="1.0.0", skill_assets=[], write_gate=True)
    hooks = json.loads(next(a for a in bundle if a.path.name == "hooks.json").content)
    assert "PreToolUse" in hooks["hooks"] and "PostToolUse" in hooks["hooks"]


def test_plugin_bundle_includes_the_output_style(tmp_path):
    """A plugin-only install must not silently lose the DevCouncil output style.

    `dev integrate claude --apply` writes `.claude/output-styles/devcouncil.md`; the bundle
    shipped nothing, so the two install paths were not equivalent. `output-styles/` is the
    plugin spec's default directory for this (plugins-reference: Output styles ->
    `output-styles/`, manifest field `outputStyles`), and Claude Code 2.1.259 lists
    `output-styles/` among the recognized plugin content directories and documents the
    manifest field as "When set, the output-styles/ directory is not auto-loaded" — so the
    default directory is auto-loaded and no manifest entry is invented here.
    """
    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0", skill_assets=[])
    rel = f"{claude_assets.PLUGIN_ROOT_REL.as_posix()}/devcouncil/output-styles/devcouncil.md"
    style = next((a for a in bundle if a.path.relative_to(tmp_path).as_posix() == rel), None)
    assert style is not None, (
        "the plugin bundle ships no output style; a plugin-only install loses it: "
        + str(sorted(a.path.relative_to(tmp_path).as_posix() for a in bundle))
    )
    # Byte-identical to the .claude/ copy — one body, two destinations.
    assert style.content == claude_assets.build_output_style(tmp_path)[0].content
    meta, _ = split_frontmatter(style.content)
    assert meta["name"] == "DevCouncil"

    # No `outputStyles` override, so Claude Code auto-loads output-styles/.
    manifest = json.loads(next(a for a in bundle if a.path.name == "plugin.json").content)
    assert "outputStyles" not in manifest


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
    assert "hook claude-statusline" in settings["statusLine"]["command"]
    assert "--project-root" in settings["statusLine"]["command"]
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
    stop_gate = config.get("execution", {}).get("stop_gate", {})
    assert stop_gate.get("mode") == "assist"
    assert stop_gate.get("check_claims") is True
    assert stop_gate.get("verify_active_task") is True


def test_codex_hooks_apply_seeds_stop_gate_assist(tmp_path):
    _init_repo(tmp_path)
    from devcouncil.integrations.clients.hooks import _configure_native_hooks
    import yaml

    _configure_native_hooks(tmp_path, tool="codex", apply=True)
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    stop_gate = config.get("execution", {}).get("stop_gate", {})
    assert stop_gate.get("mode") == "assist"
    assert stop_gate.get("check_claims") is True
    assert stop_gate.get("verify_active_task") is True


def test_stop_gate_assist_not_seeded_for_cursor_hooks(tmp_path):
    _init_repo(tmp_path)
    from devcouncil.integrations.clients.hooks import _configure_native_hooks
    import yaml

    _configure_native_hooks(tmp_path, tool="cursor", apply=True)
    config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    stop_gate = config.get("execution", {}).get("stop_gate")
    assert stop_gate is None or not stop_gate.get("mode")


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
    for sub in ("session-end", "pre-compact", "post-compact", "subagent-stop", "notification"):
        result = runner.invoke(app, ["hook", sub, "{}", "--project-root", str(tmp_path)])
        assert result.exit_code == 0, f"{sub}: {result.output}"


def test_claude_statusline_falls_back_when_uninitialized(tmp_path):
    result = runner.invoke(app, ["hook", "claude-statusline", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "not initialized" in result.stdout


def test_claude_native_hooks_assist_mode_default_has_no_write_gate(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "hooks", "--tool", "claude", "--apply", "--no-git", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    events = set(settings["hooks"].keys())
    # Assistive lifecycle hooks + refresh-only PostToolUse present...
    assert {"Stop", "SessionStart", "UserPromptSubmit", "SessionEnd", "PreCompact", "PostCompact", "SubagentStop", "Notification", "PostToolUse"} <= events
    # ...but the blocking PreToolUse write-gate is NOT installed by default.
    assert "PreToolUse" not in events


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
    assert "devcouncil_checkout_task" in result.messages[0].content.text or "Task checkout" in result.messages[0].content.text


def test_mcp_get_unknown_prompt_raises():
    from devcouncil.integrations.mcp import server

    with pytest.raises(ValueError):
        asyncio.run(server.get_prompt("does_not_exist", {}))


def test_marketplace_manifest_passes_strict_plugin_validation():
    """The generated marketplace must satisfy `claude plugin validate --strict`.

    Verified against the real validator (Claude Code 2.1.259) on a generated
    bundle::

        $ claude plugin validate <bundle>/.devcouncil/claude-plugin --strict
        ⚠ Found 1 warning:
          ❯ description: No marketplace description provided. Adding a
            description helps users understand what this marketplace offers
        ✘ Validation failed (--strict treats warnings as errors)

    The plugin manifest itself passes strict cleanly; only the marketplace was
    missing the field. `--strict` is what a publishing pipeline runs, so a
    warning here is a release blocker rather than a cosmetic note.
    """
    import json

    from devcouncil.integrations import claude_assets

    manifest = json.loads(claude_assets._marketplace_json("0.1.0"))

    description = manifest.get("description")
    assert isinstance(description, str) and description.strip(), (
        "the marketplace manifest needs a non-empty description or "
        "`claude plugin validate --strict` fails: " + repr(manifest)
    )
    # The plugin entry's own description is separate and was already present.
    assert manifest["plugins"][0]["description"]
