"""Claude integration adapter."""
from __future__ import annotations

import json
import shutil
from pathlib import Path



from devcouncil.integrations.clients import common as _common
_project_root = _common._project_root
_warn_if_verify_only = _common._warn_if_verify_only
_server_args = _common._server_args
_format_command = _common._format_command
_quote_powershell_arg = _common._quote_powershell_arg
_run = _common._run
_run_capture = _common._run_capture
_config_path = _common._config_path
_load_raw_config = _common._load_raw_config
_save_raw_config = _common._save_raw_config
_load_json = _common._load_json
_save_json = _common._save_json
_load_json_strict = _common._load_json_strict
_mutate_raw_config = _common._mutate_raw_config
_batched_raw_config = _common._batched_raw_config
_probe_mcp_tools = _common._probe_mcp_tools
_print_command = _common._print_command
_configure = _common._configure

console = _common.console

from devcouncil.integrations.clients import hooks as _hooks  # noqa: E402
_install_claude_hooks = _hooks._install_claude_hooks
def _claude_command(project_root: Path, scope: str) -> list[str]:
    # The server name must come BEFORE --env: the current Claude CLI treats --env
    # as variadic, so `--env KEY=VALUE devcouncil` swallows the name `devcouncil`
    # as a second (invalid) env var. Putting the name first — matching the working
    # codex form — and terminating options with `--` avoids that.
    return [
        "claude",
        "mcp",
        "add",
        "--scope",
        scope,
        "devcouncil",
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "--",
        *_server_args(project_root),
    ]

def _devcouncil_version() -> str:
    """Package version for plugin manifests, or a stable placeholder when uninstalled."""
    import importlib.metadata

    try:
        return importlib.metadata.version("devcouncil")
    except importlib.metadata.PackageNotFoundError:
        return "0.0.0"

_CLAUDE_OUTPUT_STYLE = "DevCouncil"


def _record_claude_config(
    project_root: Path, *, scope: str = "project", write_gate: bool = False
) -> None:
    def mutate(config: dict) -> None:
        claude = config.setdefault("integrations", {}).setdefault("claude", {})
        claude.update({
            "enabled": True,
            "scope": scope,
            "write_gate": write_gate,
            "settings_path": ".claude/settings.local.json",
        })

    _mutate_raw_config(project_root, mutate)


def _install_claude_settings(project_root: Path) -> tuple[Path, bool]:
    """Write the statusLine, MCP enablement, and permission allow-list into Claude settings.

    Merges into .claude/settings.local.json without clobbering existing user entries.
    Returns (path, changed); only rewrites the file when the merge changes something so
    re-running integration is a true no-op."""
    from devcouncil.integrations.claude_assets import claude_bash_permission_allow

    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)

    settings["statusLine"] = {
        "type": "command",
        "command": "devcouncil hook claude-statusline",
    }
    if settings.get("outputStyle") != _CLAUDE_OUTPUT_STYLE:
        settings["outputStyle"] = _CLAUDE_OUTPUT_STYLE
    # Auto-enable the project-scoped DevCouncil MCP server so a teammate cloning the repo
    # doesn't have to approve it interactively.
    enabled = settings.setdefault("enabledMcpjsonServers", [])
    if isinstance(enabled, list) and "devcouncil" not in enabled:
        enabled.append("devcouncil")

    permissions = settings.setdefault("permissions", {})
    if isinstance(permissions, dict):
        allow = permissions.setdefault("allow", [])
        if isinstance(allow, list):
            for rule in claude_bash_permission_allow():
                if rule not in allow:
                    allow.append(rule)

    changed = json.dumps(settings, sort_keys=True) != before
    if changed:
        _save_json(path, settings)
    return path, changed

def _selected_skill_assets(project_root: Path):
    """Scaffold the applicable skills and return them as GeneratedAsset-like records.

    Returns (written_paths, skill_assets) where skill_assets carry (path, content) for the
    plugin bundler so the plugin ships the same skill bodies that land in .claude/skills/."""
    from devcouncil.integrations.claude_assets import GeneratedAsset
    from devcouncil.skills.registry import scaffold_skills, select_skills

    skills = select_skills("", project_root)
    written = scaffold_skills(project_root, skills)
    assets: list[GeneratedAsset] = []
    skills_root = project_root / ".claude" / "skills"
    for skill in skills:
        target = skills_root / skill.name / "SKILL.md"
        if target.exists():
            assets.append(GeneratedAsset(target, target.read_text(encoding="utf-8")))
    return written, assets

def _install_claude_assets(project_root: Path) -> list[Path]:
    """Generate the static Claude Code asset surface (commands, agents, output style,
    statusline, permissions) and scaffold the applicable skills. Idempotent."""
    from devcouncil.integrations import claude_assets

    written: list[Path] = []
    assets: list[claude_assets.GeneratedAsset] = []
    assets += claude_assets.build_slash_commands(project_root)
    assets += claude_assets.build_subagents(project_root)
    assets += claude_assets.build_output_style(project_root)
    for asset in assets:
        if asset.write_if_changed():
            written.append(asset.path)

    skills_written, _ = _selected_skill_assets(project_root)
    written.extend(skills_written)
    settings_path, settings_changed = _install_claude_settings(project_root)
    if settings_changed:
        written.append(settings_path)
    return written

def _install_claude_plugin(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Build the self-contained Claude Code plugin + single-repo marketplace bundle.

    Bundles the commands, agents, applicable skills, hooks, and MCP config so the entire
    DevCouncil integration installs with one `/plugin install`. Assist-mode hooks by
    default; pass write_gate=True to bundle the blocking containment gate."""
    from devcouncil.integrations import claude_assets

    _, skill_assets = _selected_skill_assets(project_root)
    bundle = claude_assets.build_plugin_bundle(
        project_root, version=_devcouncil_version(), skill_assets=skill_assets, write_gate=write_gate
    )
    return [asset.path for asset in bundle if asset.write_if_changed()]

def _uninstall_claude(project_root: Path) -> list[str]:
    """Remove everything DevCouncil installed into a Claude Code project. Idempotent.

    Strips DevCouncil's hooks (every event), the DevCouncil statusLine, the MCP enablement
    and permission rules from .claude/settings.local.json (leaving any user-authored
    entries untouched), deletes the generated commands/subagents/output-style files, and
    best-effort de-registers the MCP server via `claude mcp remove`. Returns a list of the
    changes made. The recoverable, in-band counterpart to a fail-closed write-gate."""
    removed: list[str] = []
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)

    # Hooks: drop any entry whose command invokes `devcouncil hook`, then prune empties.
    hooks = settings.get("hooks")
    if isinstance(hooks, dict):
        for event in list(hooks):
            groups = hooks.get(event)
            if not isinstance(groups, list):
                continue
            kept_groups = []
            for group in groups:
                inner = group.get("hooks", []) if isinstance(group, dict) else []
                inner_kept = [
                    h for h in inner
                    if "devcouncil hook" not in str(h.get("command", ""))
                ]
                if inner_kept:
                    group["hooks"] = inner_kept
                    kept_groups.append(group)
            if kept_groups:
                hooks[event] = kept_groups
            else:
                hooks.pop(event)
        if not hooks:
            settings.pop("hooks")
        removed.append(f"hooks in {path.name}")

    # statusLine: only remove ours.
    status = settings.get("statusLine")
    if isinstance(status, dict) and "devcouncil" in str(status.get("command", "")):
        settings.pop("statusLine")
        removed.append("statusLine")

    if settings.get("outputStyle") == _CLAUDE_OUTPUT_STYLE:
        settings.pop("outputStyle", None)
        removed.append("outputStyle")

    enabled = settings.get("enabledMcpjsonServers")
    if isinstance(enabled, list) and "devcouncil" in enabled:
        enabled.remove("devcouncil")
        if not enabled:
            settings.pop("enabledMcpjsonServers")
        removed.append("enabledMcpjsonServers entry")

    permissions = settings.get("permissions")
    if isinstance(permissions, dict) and isinstance(permissions.get("allow"), list):
        from devcouncil.integrations.claude_assets import claude_bash_permission_allow

        ours = set(claude_bash_permission_allow())
        kept = [r for r in permissions["allow"] if r not in ours]
        if len(kept) != len(permissions["allow"]):
            permissions["allow"] = kept
            removed.append("permission allow-rules")
        if not permissions.get("allow"):
            permissions.pop("allow", None)
        if not permissions:
            settings.pop("permissions")

    if json.dumps(settings, sort_keys=True) != before:
        if settings:
            _save_json(path, settings)
        elif path.exists():
            path.unlink()
            removed.append(f"deleted empty {path.name}")

    # Generated asset files.
    targets = [
        project_root / ".claude" / "commands" / "devcouncil",
        project_root / ".claude" / "output-styles" / "devcouncil.md",
    ]
    targets += [
        project_root / ".claude" / "agents" / f"{name}.md"
        for name in ("devcouncil-implementer", "devcouncil-verifier", "devcouncil-reviewer")
    ]
    for target in targets:
        if target.is_dir():
            shutil.rmtree(target)
            removed.append(str(target.relative_to(project_root)))
        elif target.exists():
            target.unlink()
            removed.append(str(target.relative_to(project_root)))

    # De-register the MCP server (best-effort; only if the claude CLI is present).
    if shutil.which("claude"):
        code = _run(["claude", "mcp", "remove", "devcouncil"])
        if code == 0:
            removed.append("claude mcp server registration")

    return removed
