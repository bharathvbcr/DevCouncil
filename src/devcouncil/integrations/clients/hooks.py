"""Hooks integration adapter."""
from __future__ import annotations

import json
import re
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
_apply_decouple_config_flags = _common._apply_decouple_config_flags

console = _common.console
OPENCODE_HOOK_PLUGIN_NAME = _common.OPENCODE_HOOK_PLUGIN_NAME
SUPPORTED_HOOK_TOOLS = _common.SUPPORTED_HOOK_TOOLS

SESSION_START_MATCHER = "startup|resume|clear|compact"
GIT_MAP_HOOK_MARKER = "# DevCouncil: refresh repo map"
# Clients that install PreToolUse / BeforeTool / Cursor pre / OpenCode before containment.
CONTAINMENT_HOOK_CLIENTS = ("claude", "codex", "cursor", "grok", "opencode", "gemini")

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

def _install_codex_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Codex hooks. PostToolUse/lifecycle always; PreToolUse only with ``write_gate``.

    Assist (default) omits PreToolUse so interactive sessions are not re-bound to a
    leftover lease allowlist. Under ``write_gate`` (contain), PreToolUse is installed;
    Codex exit-2 deny on that path is intentional for autonomous runs.
    """
    path = project_root / ".codex" / "hooks.json"
    settings = _load_json(path)
    matcher = "Bash|shell_command|exec_command|local_shell|Write|Edit|MultiEdit|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "codex", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    if write_gate:
        _upsert_hook(
            settings,
            "PreToolUse",
            matcher,
            _hook_command(project_root, "codex", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
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

    def mutate(config: dict) -> None:
        codex = config.setdefault("integrations", {}).setdefault("codex", {})
        if isinstance(codex, dict):
            codex["write_gate"] = write_gate
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path, _ensure_codex_hooks_enabled(project_root)]

def _install_gemini_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Gemini hooks. AfterTool always; BeforeTool only with ``write_gate``."""
    path = project_root / ".gemini" / "settings.json"
    settings = _load_json(path)
    matcher = "run_shell_command|shell_command|write_file|edit_file|replace|apply_patch"
    _upsert_hook(
        settings,
        "AfterTool",
        matcher,
        _hook_command(project_root, "gemini", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    if write_gate:
        _upsert_hook(
            settings,
            "BeforeTool",
            matcher,
            _hook_command(project_root, "gemini", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        _remove_named_hook(settings, "BeforeTool", "devcouncil-pre-tool-use")
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        gemini = config.setdefault("integrations", {}).setdefault("gemini", {})
        if isinstance(gemini, dict):
            gemini["write_gate"] = write_gate
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path]

def _upsert_cursor_hook(settings: dict, event: str, matcher: str, command: str) -> None:
    hooks = settings.setdefault("hooks", {})
    entries = hooks.setdefault(event, [])
    for entry in entries:
        if entry.get("command") == command:
            # Refresh matcher on re-apply (e.g. drop Read|Task).
            if matcher:
                entry["matcher"] = matcher
            elif "matcher" in entry:
                entry.pop("matcher", None)
            return
    payload: dict = {"command": command}
    if matcher:
        payload["matcher"] = matcher
    entries.append(payload)


def _remove_cursor_hook(settings: dict, event: str, *, command_substr: str) -> None:
    """Remove Cursor hook entries whose command contains ``command_substr``."""
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return
    entries = hooks.get(event)
    if not isinstance(entries, list):
        return
    kept = [
        entry
        for entry in entries
        if not (isinstance(entry, dict) and command_substr in str(entry.get("command") or ""))
    ]
    if kept:
        hooks[event] = kept
    else:
        hooks.pop(event, None)


def _install_cursor_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Cursor hooks. PostToolUse always; PreToolUse only with ``write_gate``."""
    root = project_root.expanduser().resolve()
    path = root / ".cursor" / "hooks.json"
    settings = _load_json(path)
    settings.setdefault("version", 1)
    matcher = "Shell|Write|Edit|MultiEdit"
    post_cmd = _hook_command(root, "cursor", "post-tool-use")
    pre_cmd = _hook_command(root, "cursor", "pre-tool-use")
    # Drop prior DevCouncil entries (absolute/relative path drift) before upsert.
    _remove_cursor_hook(settings, "postToolUse", command_substr="post-tool-use")
    _remove_cursor_hook(settings, "preToolUse", command_substr="pre-tool-use")
    _upsert_cursor_hook(settings, "postToolUse", matcher, post_cmd)
    if write_gate:
        _upsert_cursor_hook(settings, "preToolUse", matcher, pre_cmd)
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "hooks_path": str(path.relative_to(root)),
            "write_gate": write_gate,
        })
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(root, mutate)
    return [path]


def _install_grok_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Grok hooks. PostToolUse always; PreToolUse only with ``write_gate``."""
    hooks_dir = project_root / ".grok" / "hooks"
    hooks_dir.mkdir(parents=True, exist_ok=True)
    path = hooks_dir / "devcouncil.json"
    settings = _load_json(path)
    matcher = "Bash|Write|Edit|MultiEdit|run_terminal_cmd|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "grok", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    if write_gate:
        _upsert_hook(
            settings,
            "PreToolUse",
            matcher,
            _hook_command(project_root, "grok", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        grok = config.setdefault("integrations", {}).setdefault("grok", {})
        grok.update({
            "hooks_path": str(path.relative_to(project_root)),
            "write_gate": write_gate,
        })
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path]


def _opencode_plugin_body(*, write_gate: bool) -> str:
    """Generate OpenCode plugin source. Pre-tool gate only when write_gate."""
    lines = [
        'import { spawnSync } from "node:child_process";',
        "",
        "const projectRoot = process.env.DEVCOUNCIL_PROJECT_ROOT || process.cwd();",
        "",
        "function runHook(event, payload) {",
        '  const args = ["hook", event, "--client", "opencode", "--project-root", projectRoot];',
        "  const result = spawnSync(\"devcouncil\", args, {",
        "    input: JSON.stringify(payload ?? {}),",
        '    encoding: "utf-8",',
        "    env: { ...process.env, DEVCOUNCIL_PROJECT_ROOT: projectRoot },",
        "  });",
        "  if (result.status === 2) {",
        '    throw new Error(result.stderr || result.stdout || "DevCouncil blocked the tool call.");',
        "  }",
        "}",
        "",
        "export const DevCouncilOpenCodeHook = async () => ({",
    ]
    if write_gate:
        lines.extend(
            [
                '  "tool.execute.before": async (input, output) => {',
                "    runHook(\"pre-tool-use\", { tool: input.tool, arguments: output.args });",
                "  },",
            ]
        )
    lines.extend(
        [
            '  "tool.execute.after": async (input, output) => {',
            "    runHook(\"post-tool-use\", { tool: input.tool, arguments: output.args });",
            "  },",
            "});",
            "",
        ]
    )
    return "\n".join(lines)


def _install_opencode_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    destination = _opencode_plugin_path(project_root)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(_opencode_plugin_body(write_gate=write_gate), encoding="utf-8")

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

    def mutate(config: dict) -> None:
        opencode = config.setdefault("integrations", {}).setdefault("opencode", {})
        opencode.update({"write_gate": write_gate})
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
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
    # Keep runtime gate posture and assist/contain flag in sync with --write-gate.
    def mutate(config: dict) -> None:
        claude = config.setdefault("integrations", {}).setdefault("claude", {})
        if isinstance(claude, dict):
            claude["write_gate"] = write_gate
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
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
    project_root: Path, tool: str = "all", apply: bool = False, *, write_gate: bool = False, claude_write_gate: bool | None = None
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
    # Prefer write_gate; accept legacy claude_write_gate kw from older callers.
    if claude_write_gate is not None:
        write_gate = bool(claude_write_gate)

    installers = {
        # Blocking PreToolUse/BeforeTool write-gate is opt-in for all clients.
        "codex": lambda root: _install_codex_hooks(root, write_gate=write_gate),
        "gemini": lambda root: _install_gemini_hooks(root, write_gate=write_gate),
        "claude": lambda root: _install_claude_hooks(root, write_gate=write_gate),
        "cursor": lambda root: _install_cursor_hooks(root, write_gate=write_gate),
        "grok": lambda root: _install_grok_hooks(root, write_gate=write_gate),
        "opencode": lambda root: _install_opencode_hooks(root, write_gate=write_gate),
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


def _strip_devcouncil_hooks_from_settings(settings: dict) -> bool:
    """Remove every hook entry whose command invokes ``devcouncil hook`` / ``dev hook``.

    Returns True when settings were mutated. Prunes empty matcher groups and events.
    """
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return False
    changed = False
    for event in list(hooks):
        groups = hooks.get(event)
        if not isinstance(groups, list):
            continue
        # Claude/Codex/Gemini/Grok: list of matcher groups with nested hooks.
        if groups and isinstance(groups[0], dict) and "hooks" in groups[0]:
            kept_groups: list[dict] = []
            for group in groups:
                if not isinstance(group, dict):
                    kept_groups.append(group)
                    continue
                inner = group.get("hooks", [])
                if not isinstance(inner, list):
                    kept_groups.append(group)
                    continue
                inner_kept = [
                    h for h in inner
                    if not (
                        isinstance(h, dict)
                        and (
                            "devcouncil hook" in str(h.get("command", ""))
                            or str(h.get("name", "")).startswith("devcouncil-")
                            or re.search(r"(^|[\s/\\])dev(\.exe)?\s+hook\b", str(h.get("command", "")))
                        )
                    )
                ]
                if len(inner_kept) != len(inner):
                    changed = True
                if inner_kept:
                    group = {**group, "hooks": inner_kept}
                    kept_groups.append(group)
            if kept_groups:
                if hooks.get(event) != kept_groups:
                    hooks[event] = kept_groups
                    changed = True
            else:
                hooks.pop(event, None)
                changed = True
            continue
        # Cursor-style: flat list of {command, matcher?} entries.
        kept_entries = [
            entry
            for entry in groups
            if not (
                isinstance(entry, dict)
                and (
                    "devcouncil hook" in str(entry.get("command", ""))
                    or "pre-tool-use" in str(entry.get("command", ""))
                    or "post-tool-use" in str(entry.get("command", ""))
                    or re.search(r"(^|[\s/\\])dev(\.exe)?\s+hook\b", str(entry.get("command", "")))
                )
            )
        ]
        if len(kept_entries) != len(groups):
            changed = True
            if kept_entries:
                hooks[event] = kept_entries
            else:
                hooks.pop(event, None)
    if not hooks:
        settings.pop("hooks", None)
        changed = True
    return changed


def _strip_opencode_before_handler_text(text: str) -> tuple[str, bool]:
    """Surgically drop ``tool.execute.before`` from an OpenCode plugin body.

    Returns (new_text, stripped). Falls back to a full assist-mode regen when the
    before-handler shape cannot be parsed safely.
    """
    marker = '"tool.execute.before"'
    if marker not in text:
        return text, False
    known_contain = _opencode_plugin_body(write_gate=True)
    known_assist = _opencode_plugin_body(write_gate=False)
    if text == known_contain:
        return known_assist, True
    idx = text.find(marker)
    # Walk back to the start of the property line.
    line_start = text.rfind("\n", 0, idx) + 1
    # Find the opening brace of the async handler body after `=>`.
    arrow = text.find("=>", idx)
    if arrow < 0:
        return known_assist, True
    brace = text.find("{", arrow)
    if brace < 0:
        return known_assist, True
    depth = 0
    end = brace
    for i in range(brace, len(text)):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    else:
        return known_assist, True
    # Consume trailing comma + whitespace/newlines after the handler.
    while end < len(text) and text[end] in " \t":
        end += 1
    if end < len(text) and text[end] == ",":
        end += 1
    while end < len(text) and text[end] in " \t\r":
        end += 1
    if end < len(text) and text[end] == "\n":
        end += 1
    stripped = text[:line_start] + text[end:]
    if marker in stripped:
        return known_assist, True
    return stripped, True


def _strip_command_hooks_from_event(
    settings: dict,
    event: str,
    *,
    command_substr: str,
) -> None:
    """Drop nested hook handlers whose command contains ``command_substr``; prune empties."""
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return
    groups = hooks.get(event)
    if not isinstance(groups, list):
        return
    kept_groups: list[dict] = []
    for group in groups:
        if not isinstance(group, dict):
            kept_groups.append(group)
            continue
        inner = group.get("hooks", [])
        if not isinstance(inner, list):
            kept_groups.append(group)
            continue
        inner_kept = [
            h
            for h in inner
            if not (isinstance(h, dict) and command_substr in str(h.get("command", "")))
        ]
        if inner_kept:
            kept_groups.append({**group, "hooks": inner_kept})
    if kept_groups:
        hooks[event] = kept_groups
    else:
        hooks.pop(event, None)
    if not hooks:
        settings.pop("hooks", None)


def _strip_claude_containment(project_root: Path) -> list[str]:
    """Remove PreToolUse containment only; leave PostToolUse/lifecycle hooks intact."""
    path = project_root / ".claude" / "settings.local.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    # Also drop unnamed PreToolUse entries that still invoke the pre-tool-use hook.
    _strip_command_hooks_from_event(settings, "PreToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    if settings:
        _save_json(path, settings)
    elif path.exists():
        path.unlink()
    return [f"stripped PreToolUse from {path.name}"]


def _strip_codex_containment(project_root: Path) -> list[str]:
    path = project_root / ".codex" / "hooks.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _strip_command_hooks_from_event(settings, "PreToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    _save_json(path, settings)
    return [f"stripped PreToolUse from {path.relative_to(project_root)}"]


def _strip_gemini_containment(project_root: Path) -> list[str]:
    path = project_root / ".gemini" / "settings.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "BeforeTool", "devcouncil-pre-tool-use")
    _strip_command_hooks_from_event(settings, "BeforeTool", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    _save_json(path, settings)
    return [f"stripped BeforeTool from {path.relative_to(project_root)}"]


def _strip_cursor_containment(project_root: Path) -> list[str]:
    path = project_root / ".cursor" / "hooks.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_cursor_hook(settings, "preToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    hooks = settings.get("hooks")
    if isinstance(hooks, dict) and not hooks:
        settings.pop("hooks", None)
    if settings.get("hooks") or settings.get("version") is not None:
        _save_json(path, settings)
    elif path.exists():
        path.unlink()
    return [f"stripped preToolUse from {path.relative_to(project_root)}"]


def _strip_grok_containment(project_root: Path) -> list[str]:
    path = project_root / ".grok" / "hooks" / "devcouncil.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _strip_command_hooks_from_event(settings, "PreToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    _save_json(path, settings)
    return [f"stripped PreToolUse from {path.relative_to(project_root)}"]


def _strip_opencode_containment(project_root: Path) -> list[str]:
    path = _opencode_plugin_path(project_root)
    if not path.exists():
        return []
    text = path.read_text(encoding="utf-8")
    new_text, stripped = _strip_opencode_before_handler_text(text)
    if not stripped or new_text == text:
        return []
    path.write_text(new_text, encoding="utf-8")
    return [f"stripped tool.execute.before from {path.relative_to(project_root)}"]


_CONTAINMENT_STRIPPERS = {
    "claude": _strip_claude_containment,
    "codex": _strip_codex_containment,
    "gemini": _strip_gemini_containment,
    "cursor": _strip_cursor_containment,
    "grok": _strip_grok_containment,
    "opencode": _strip_opencode_containment,
}


def _decouple_client_hooks(project_root: Path, client: str) -> list[str]:
    """Strip containment hooks for one client and force assist config flags."""
    stripper = _CONTAINMENT_STRIPPERS.get(client)
    changes: list[str] = []
    if stripper is not None:
        changes.extend(stripper(project_root))
    changes.extend(_apply_decouple_config_flags(project_root, client if client in _common.CLIENTS_WITH_WRITE_GATE else None))
    return changes


def _decouple_all_containment_hooks(project_root: Path) -> list[str]:
    """Strip PreToolUse/before containment across all hook clients (no reinstall)."""
    changes: list[str] = []
    with _batched_raw_config(project_root):
        for client in CONTAINMENT_HOOK_CLIENTS:
            changes.extend(_CONTAINMENT_STRIPPERS[client](project_root))

        def mutate(config: dict) -> None:
            for client in _common.CLIENTS_WITH_WRITE_GATE:
                entry = config.setdefault("integrations", {}).setdefault(client, {})
                if isinstance(entry, dict) and entry.get("write_gate") is not False:
                    entry["write_gate"] = False
                    changes.append(f"integrations.{client}.write_gate=false")
            execution = config.setdefault("execution", {})
            hook_gate = execution.setdefault("hook_gate", {})
            if not isinstance(hook_gate, dict):
                hook_gate = {}
                execution["hook_gate"] = hook_gate
            if hook_gate.get("mode") != "off":
                hook_gate["mode"] = "off"
                changes.append("execution.hook_gate.mode=off")
            stop_gate = execution.setdefault("stop_gate", {})
            if not isinstance(stop_gate, dict):
                stop_gate = {}
                execution["stop_gate"] = stop_gate
            mode = str(stop_gate.get("mode") or "").strip().lower()
            if mode == "block":
                stop_gate["mode"] = "assist"
                changes.append("execution.stop_gate.mode=assist")

        _mutate_raw_config(project_root, mutate)
    seen: set[str] = set()
    unique: list[str] = []
    for item in changes:
        if item not in seen:
            seen.add(item)
            unique.append(item)
    return unique


def _uninstall_named_hook_file(path: Path, project_root: Path, *, delete_if_empty: bool = True) -> list[str]:
    """Strip all DevCouncil hooks from a Claude/Codex-style hooks JSON file."""
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _strip_devcouncil_hooks_from_settings(settings)
    if json.dumps(settings, sort_keys=True) == before:
        return []
    rel = str(path.relative_to(project_root)) if path.is_relative_to(project_root) else str(path)
    if not settings and delete_if_empty:
        path.unlink()
        return [f"deleted empty {rel}"]
    # Cursor: delete when only version remains and hooks gone.
    if delete_if_empty and set(settings.keys()) <= {"version"} and "hooks" not in settings:
        path.unlink()
        return [f"deleted empty {rel}"]
    _save_json(path, settings)
    return [f"stripped DevCouncil hooks from {rel}"]


def _uninstall_claude_hooks(project_root: Path) -> list[str]:
    return _uninstall_named_hook_file(project_root / ".claude" / "settings.local.json", project_root, delete_if_empty=False)


def _uninstall_codex_hooks(project_root: Path) -> list[str]:
    # Do not touch [features] hooks in config.toml — only strip hooks.json entries.
    return _uninstall_named_hook_file(project_root / ".codex" / "hooks.json", project_root)


def _uninstall_gemini_hooks(project_root: Path) -> list[str]:
    return _uninstall_named_hook_file(project_root / ".gemini" / "settings.json", project_root, delete_if_empty=False)


def _uninstall_cursor_hooks(project_root: Path) -> list[str]:
    return _uninstall_named_hook_file(project_root / ".cursor" / "hooks.json", project_root)


def _uninstall_grok_hooks(project_root: Path) -> list[str]:
    path = project_root / ".grok" / "hooks" / "devcouncil.json"
    if not path.exists():
        return []
    path.unlink()
    return [str(path.relative_to(project_root))]


def _uninstall_opencode_hooks(project_root: Path) -> list[str]:
    """Remove the generated OpenCode hook plugin file and its plugin[] registration.

    Leaves ``mcp.devcouncil`` alone — that is full client uninstall territory.
    """
    removed: list[str] = []
    root = project_root.expanduser().resolve()
    plugin = _opencode_plugin_path(root)
    if plugin.exists():
        plugin.unlink()
        removed.append(str(plugin.relative_to(root)))
    path = _opencode_config_path(root)
    if path.exists():
        data = _load_json(path)
        plugins = data.get("plugin")
        changed = False
        if isinstance(plugins, list):
            kept = [p for p in plugins if not _opencode._is_opencode_plugin_ref(p)]
            if len(kept) != len(plugins):
                if kept:
                    data["plugin"] = kept
                else:
                    data.pop("plugin", None)
                changed = True
                removed.append(f"plugin entry {_opencode._opencode_plugin_ref_canonical()}")
        elif isinstance(plugins, str) and _opencode._is_opencode_plugin_ref(plugins):
            data.pop("plugin", None)
            changed = True
            removed.append(f"plugin entry {_opencode._opencode_plugin_ref_canonical()}")
        if changed:
            if data:
                _save_json(path, data)
            elif path.exists():
                path.unlink()
    return removed


def _strip_git_map_hook_block(text: str, marker: str = GIT_MAP_HOOK_MARKER) -> str:
    """Remove DevCouncil map-refresh marker + following command line from a git hook."""
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    i = 0
    while i < len(lines):
        if marker in lines[i]:
            i += 1
            # Skip blank lines immediately after the marker, then the map command.
            while i < len(lines) and not lines[i].strip():
                i += 1
            if i < len(lines) and ("map --if-stale" in lines[i] or "map --no-wiki" in lines[i]):
                i += 1
            continue
        out.append(lines[i])
        i += 1
    return "".join(out)


def _uninstall_git_map_hooks(project_root: Path) -> list[str]:
    """Reverse ``_install_git_map_hooks`` — remove DevCouncil map-refresh blocks only."""
    root = project_root.expanduser().resolve()
    hooks_dir = root / ".git" / "hooks"
    if not hooks_dir.is_dir():
        return []
    removed: list[str] = []
    for name in ("post-commit", "post-merge", "post-checkout"):
        path = hooks_dir / name
        if not path.exists():
            continue
        existing = path.read_text(encoding="utf-8")
        if GIT_MAP_HOOK_MARKER not in existing:
            continue
        updated = _strip_git_map_hook_block(existing)
        rel = str(path.relative_to(root)) if path.is_relative_to(root) else str(path)
        # Drop file when only a shebang / whitespace remains.
        residual = updated.strip()
        if residual in ("", "#!/bin/sh", "#!/usr/bin/env bash", "#!/bin/bash"):
            path.unlink()
            removed.append(f"deleted {rel}")
        else:
            path.write_text(updated, encoding="utf-8")
            removed.append(f"stripped DevCouncil map block from {rel}")
    return removed


_NATIVE_HOOK_UNINSTALLERS = {
    "claude": _uninstall_claude_hooks,
    "codex": _uninstall_codex_hooks,
    "gemini": _uninstall_gemini_hooks,
    "cursor": _uninstall_cursor_hooks,
    "grok": _uninstall_grok_hooks,
    "opencode": _uninstall_opencode_hooks,
}


def _uninstall_native_hooks_for_tool(project_root: Path, tool: str) -> list[str]:
    """Uninstall DevCouncil hooks for one native hook tool (no git map hooks)."""
    uninstall = _NATIVE_HOOK_UNINSTALLERS.get(tool)
    if uninstall is None:
        raise ValueError(f"Unsupported hook tool '{tool}'.")
    return uninstall(project_root)


def _uninstall_all_native_hooks(project_root: Path, *, include_git: bool = True) -> list[str]:
    """Uninstall hook files/entries for every hook tool (+ optional git map hooks)."""
    removed: list[str] = []
    for uninstall in _NATIVE_HOOK_UNINSTALLERS.values():
        removed.extend(uninstall(project_root))
    if include_git:
        removed.extend(_uninstall_git_map_hooks(project_root))
    return removed

