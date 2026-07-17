"""Hooks integration adapter."""
from __future__ import annotations

from pathlib import Path

import typer

from devcouncil.executors.agent_registry import GEMINI_DEPRECATION_MESSAGE
from devcouncil.integrations.clients import common as _common
from devcouncil.integrations.clients import opencode as _opencode
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
resolve_dev_executable = _common.resolve_dev_executable
record_hook_dev_executable = _common.record_hook_dev_executable
check_hook_dev_executable = _common.check_hook_dev_executable

console = _common.console
OPENCODE_HOOK_PLUGIN_NAME = _common.OPENCODE_HOOK_PLUGIN_NAME
SUPPORTED_HOOK_TOOLS = _common.SUPPORTED_HOOK_TOOLS

SESSION_START_MATCHER = "startup|resume|clear|compact"

_opencode_plugin_path = _opencode._opencode_plugin_path
_opencode_plugin_source = _opencode._opencode_plugin_source
_opencode_config_path = _opencode._opencode_config_path
_record_opencode_config = _opencode._record_opencode_config

def _stop_hook_timeout_seconds(project_root: Path) -> int:
    """Allow up to 150 seconds when the stop gate runs claims + verification."""
    try:
        from devcouncil.app.config import load_config

        sg = load_config(project_root).execution.stop_gate
        mode = (sg.mode or "off").strip().lower()
        if mode != "off" and (sg.check_claims or sg.verify_active_task):
            return 150
    except Exception:
        pass
    return 10


def _hook_command(project_root: Path, client: str, event: str) -> str:
    # Absolute path to project-venv (or PATH) `dev` so a stale global install cannot
    # shadow the repo's CLI from PostToolUse / PreToolUse hooks.
    executable = resolve_dev_executable(project_root)
    return _format_command([
        executable,
        "hook",
        event,
        "--client",
        client,
        "--project-root",
        str(project_root),
    ])

def _upsert_hook(settings: dict, event: str, matcher: str, command: str, name: str, *, timeout: int = 10) -> None:
    hooks = settings.setdefault("hooks", {})
    groups = hooks.setdefault(event, [])
    target_group = None
    for group in groups:
        if group.get("matcher") == matcher:
            target_group = group
            break

    hook_payload = {
        "type": "command",
        "name": name,
        "command": command,
        "timeout": timeout,
    }

    if target_group is None:
        groups.append({"matcher": matcher, "hooks": [hook_payload]})
        return

    group_hooks = target_group.setdefault("hooks", [])
    replaced = False
    kept: list[dict] = []
    for hook in group_hooks:
        if hook.get("name") == name:
            if not replaced:
                # DevCouncil owns named hooks, including their timeout.  Re-applying
                # integration therefore migrates old millisecond values to the
                # seconds expected by Claude Code and Codex.
                kept.append(hook_payload)
                replaced = True
            continue
        kept.append(hook)
    if not replaced:
        kept.append(hook_payload)
    target_group["hooks"] = kept


def _remove_named_hook(settings: dict, event: str, name: str) -> None:
    """Remove a DevCouncil-owned hook and prune empty matcher groups/events."""
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return
    groups = hooks.get(event)
    if not isinstance(groups, list):
        return
    kept_groups: list[dict] = []
    for group in groups:
        if not isinstance(group, dict):
            continue
        handlers = group.get("hooks")
        if isinstance(handlers, list):
            group = {
                **group,
                "hooks": [hook for hook in handlers if not isinstance(hook, dict) or hook.get("name") != name],
            }
        if group.get("hooks"):
            kept_groups.append(group)
    if kept_groups:
        hooks[event] = kept_groups
    else:
        hooks.pop(event, None)

def _ensure_codex_hooks_enabled(project_root: Path) -> Path:
    config_path = project_root / ".codex" / "config.toml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    existing = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
    lines = existing.splitlines()
    updated: list[str] = []
    in_features = False
    saw_features = False
    inserted = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            if in_features and not inserted:
                updated.append("hooks = true")
                inserted = True
            in_features = stripped == "[features]"
            saw_features = saw_features or in_features
            updated.append(line)
            continue
        if in_features:
            key = stripped.split("=", 1)[0].strip() if "=" in stripped else ""
            if key in {"hooks", "codex_hooks"}:
                if not inserted:
                    updated.append("hooks = true")
                    inserted = True
                continue
        updated.append(line)
    if in_features and not inserted:
        updated.append("hooks = true")
        inserted = True
    if not saw_features:
        if updated and updated[-1].strip():
            updated.append("")
        updated.extend(["[features]", "hooks = true"])
    rendered = "\n".join(updated) + "\n"
    if rendered != existing:
        config_path.write_text(rendered, encoding="utf-8")
    return config_path

def _install_codex_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".codex" / "hooks.json"
    settings = _load_json(path)
    matcher = "Bash|shell_command|exec_command|local_shell|Write|Edit|MultiEdit|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PreToolUse",
        matcher,
        _hook_command(project_root, "codex", "pre-tool-use"),
        "devcouncil-pre-tool-use",
    )
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "codex", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    _upsert_hook(
        settings,
        "SessionStart",
        "startup|resume|clear|compact",
        _hook_command(project_root, "codex", "session-start"),
        "devcouncil-session-start",
    )
    _upsert_hook(
        settings,
        "Stop",
        "",
        _hook_command(project_root, "codex", "agent-response"),
        "devcouncil-agent-response-ready",
        timeout=_stop_hook_timeout_seconds(project_root),
    )
    _upsert_hook(
        settings,
        "SubagentStop",
        "",
        _hook_command(project_root, "codex", "subagent-stop"),
        "devcouncil-subagent-stop",
        timeout=_stop_hook_timeout_seconds(project_root),
    )
    _save_json(path, settings)
    return [path, _ensure_codex_hooks_enabled(project_root)]

def _install_gemini_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".gemini" / "settings.json"
    settings = _load_json(path)
    matcher = "run_shell_command|shell_command|write_file|edit_file|replace|apply_patch"
    _upsert_hook(
        settings,
        "BeforeTool",
        matcher,
        _hook_command(project_root, "gemini", "pre-tool-use"),
        "devcouncil-pre-tool-use",
    )
    _upsert_hook(
        settings,
        "AfterTool",
        matcher,
        _hook_command(project_root, "gemini", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    _save_json(path, settings)
    return [path]

def _upsert_cursor_hook(settings: dict, event: str, matcher: str, command: str) -> None:
    hooks = settings.setdefault("hooks", {})
    entries = hooks.setdefault(event, [])
    for entry in entries:
        if entry.get("command") == command:
            return
    payload: dict = {"command": command}
    if matcher:
        payload["matcher"] = matcher
    entries.append(payload)

def _install_cursor_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".cursor" / "hooks.json"
    settings = _load_json(path)
    settings.setdefault("version", 1)
    matcher = "Shell|Write|Edit|MultiEdit|Read|Task"
    _upsert_cursor_hook(
        settings,
        "preToolUse",
        matcher,
        _hook_command(project_root, "cursor", "pre-tool-use"),
    )
    _upsert_cursor_hook(
        settings,
        "postToolUse",
        matcher,
        _hook_command(project_root, "cursor", "post-tool-use"),
    )
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "hooks_path": str(path.relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)
    return [path]

def _install_grok_hooks(project_root: Path) -> list[Path]:
    hooks_dir = project_root / ".grok" / "hooks"
    hooks_dir.mkdir(parents=True, exist_ok=True)
    path = hooks_dir / "devcouncil.json"
    settings = _load_json(path)
    matcher = "Bash|Write|Edit|MultiEdit|run_terminal_cmd|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PreToolUse",
        matcher,
        _hook_command(project_root, "grok", "pre-tool-use"),
        "devcouncil-pre-tool-use",
    )
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "grok", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        grok = config.setdefault("integrations", {}).setdefault("grok", {})
        grok.update({
            "hooks_path": str(path.relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)
    return [path]

def _install_opencode_hooks(project_root: Path) -> list[Path]:
    source = _opencode_plugin_source()
    if not source.exists():
        raise FileNotFoundError(f"Missing bundled OpenCode hook plugin: {source}")
    destination = _opencode_plugin_path(project_root)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")

    path = _opencode_config_path(project_root)
    data = _load_json_strict(path, "OpenCode") if path.exists() else {"$schema": "https://opencode.ai/config.json"}
    data.setdefault("$schema", "https://opencode.ai/config.json")
    plugins_raw = data.setdefault("plugin", [])
    if not isinstance(plugins_raw, list):
        plugins_raw = []
        data["plugin"] = plugins_raw
    plugins: list[str] = [str(item) for item in plugins_raw]
    data["plugin"] = plugins
    plugin_ref = f"./.devcouncil/integrations/{OPENCODE_HOOK_PLUGIN_NAME}"
    if plugin_ref not in plugins:
        plugins.append(plugin_ref)
    _save_json(path, data)
    _record_opencode_config(project_root)
    return [destination, path]

def _install_claude_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install DevCouncil's Claude Code hooks into .claude/settings.local.json.

    By default this installs *assistive* lifecycle hooks plus a refresh-only
    **PostToolUse** hook (map auto-refresh; never gates writes). These never block
    a tool call.

    The blocking pre-action **write-gate** (**PreToolUse**, which denies any
    Bash/Write/Edit not authorized by an active task lease) is installed ONLY when
    ``write_gate`` is True. It is meant for autonomous executor runs, not interactive
    human sessions — in an interactive session there is no task lease, so the gate would
    fail-closed and deny every command. (``dev run --executor claude`` does its own
    post-hoc scope enforcement and does not depend on this hook, so leaving PreToolUse
    off by default loses no containment.)"""
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    matcher = "Bash|Write|Edit|MultiEdit"
    # Refresh-only PostToolUse is always installed so assist mode keeps the map warm.
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "claude", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    if write_gate:
        _upsert_hook(
            settings,
            "PreToolUse",
            matcher,
            _hook_command(project_root, "claude", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        # Reapplying the default assist integration must actually disable a
        # previously opted-in blocking gate; otherwise interactive sessions stay
        # fail-closed forever despite --no-write-gate.
        _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _upsert_hook(
        settings,
        "Stop",
        "",
        _hook_command(project_root, "claude", "agent-response"),
        "devcouncil-agent-response-ready",
        timeout=_stop_hook_timeout_seconds(project_root),
    )
    # Lifecycle events: status-on-start/prompt, teardown, compaction, subagent finish,
    # and notifications. These complete DevCouncil's coverage of the documented Claude
    # Code hook surface beyond the pre/post/stop gate.
    _upsert_hook(
        settings,
        "SessionStart",
        SESSION_START_MATCHER,
        _hook_command(project_root, "claude", "session-start"),
        "devcouncil-session-start",
    )
    _upsert_hook(
        settings,
        "UserPromptSubmit",
        "",
        _hook_command(project_root, "claude", "user-prompt-submit"),
        "devcouncil-user-prompt-submit",
    )
    _upsert_hook(
        settings,
        "SessionEnd",
        "",
        _hook_command(project_root, "claude", "session-end"),
        "devcouncil-session-end",
    )
    _upsert_hook(
        settings,
        "PreCompact",
        "",
        _hook_command(project_root, "claude", "pre-compact"),
        "devcouncil-pre-compact",
    )
    _upsert_hook(
        settings,
        "PostCompact",
        "",
        _hook_command(project_root, "claude", "post-compact"),
        "devcouncil-post-compact",
    )
    _upsert_hook(
        settings,
        "SubagentStop",
        "",
        _hook_command(project_root, "claude", "subagent-stop"),
        "devcouncil-subagent-stop",
        timeout=_stop_hook_timeout_seconds(project_root),
    )
    _upsert_hook(
        settings,
        "Notification",
        "",
        _hook_command(project_root, "claude", "notification"),
        "devcouncil-notification",
    )
    _save_json(path, settings)
    return [path]

def _preview_hook_paths(project_root: Path, tool: str) -> list[tuple[str, Path]]:
    paths = {
        "codex": [project_root / ".codex" / "hooks.json", project_root / ".codex" / "config.toml"],
        "gemini": [project_root / ".gemini" / "settings.json"],
        "claude": [project_root / ".claude" / "settings.local.json"],
        "cursor": [project_root / ".cursor" / "hooks.json"],
        "grok": [project_root / ".grok" / "hooks" / "devcouncil.json"],
        "opencode": [_opencode_plugin_path(project_root), _opencode_config_path(project_root)],
    }
    selected: tuple[str, ...]
    if tool == "all":
        selected = (*SUPPORTED_HOOK_TOOLS, "opencode")
    elif tool == "opencode":
        selected = ("opencode",)
    else:
        selected = (tool,)
    return [(client, path) for client in selected for path in paths.get(client, [])]

def _configure_native_hooks(
    project_root: Path, tool: str = "all", apply: bool = False, *, claude_write_gate: bool = False
) -> None:
    allowed = {"all", *SUPPORTED_HOOK_TOOLS, "gemini", "opencode"}
    if tool not in allowed:
        console.print("[red]--tool must be one of: all, codex, gemini, claude, cursor, grok, opencode.[/red]")
        raise typer.Exit(code=2)

    if tool == "gemini":
        console.print(f"[yellow]{GEMINI_DEPRECATION_MESSAGE}[/yellow]")

    if not apply:
        console.print("[bold]Native hook config preview[/bold]")
        for client, path in _preview_hook_paths(project_root, tool):
            console.print(f"{client}: {path}", soft_wrap=True)
        console.print("[yellow]Preview only. Rerun with --apply to write hook config files.[/yellow]")
        return

    selected: tuple[str, ...]
    if tool == "all":
        selected = (*SUPPORTED_HOOK_TOOLS, "opencode")
    elif tool == "opencode":
        selected = ("opencode",)
    else:
        selected = (tool,)
    installers = {
        "codex": _install_codex_hooks,
        "gemini": _install_gemini_hooks,
        # Claude's blocking write-gate is opt-in (assist-mode default); the other clients
        # install their native pre/post hooks unconditionally as before.
        "claude": lambda root: _install_claude_hooks(root, write_gate=claude_write_gate),
        "cursor": _install_cursor_hooks,
        "grok": _install_grok_hooks,
        "opencode": _install_opencode_hooks,
    }
    # Batch the per-installer config.yaml record updates (cursor/opencode)
    # into one load/save instead of re-parsing YAML per tool.
    with _batched_raw_config(project_root):
        for client in selected:
            try:
                written = installers[client](project_root)
            except (ValueError, FileNotFoundError) as exc:
                console.print(f"[red]{client} hook setup failed: {exc}[/red]")
                raise typer.Exit(code=1) from exc
            console.print(f"[green]{client} native hooks configured:[/green] {', '.join(str(path) for path in written)}")
        if any(client in _common.STOP_HOOK_TOOLS for client in selected):
            _mutate_raw_config(project_root, _common.seed_stop_gate_assist_if_unset)
    record_hook_dev_executable(project_root)
