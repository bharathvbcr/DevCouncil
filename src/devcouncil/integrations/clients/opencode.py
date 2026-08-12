"""Opencode integration adapter."""
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
OPENCODE_HOOK_PLUGIN_NAME = _common.OPENCODE_HOOK_PLUGIN_NAME

def _opencode_config_path(project_root: Path) -> Path:
    return project_root / "opencode.json"

def _opencode_mcp_entry(project_root: Path) -> dict:
    return {
        "type": "local",
        "command": ["devcouncil", "mcp-server"],
        "environment": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
        "enabled": True,
        "timeout": 10000,
    }

def _record_opencode_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        opencode = config.setdefault("integrations", {}).setdefault("opencode", {})
        opencode.update({
            "enabled": True,
            "config_path": str(_opencode_config_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)

def _write_opencode_config(project_root: Path) -> Path:
    path = _opencode_config_path(project_root)
    data = _load_json_strict(path, "OpenCode")
    data.setdefault("$schema", "https://opencode.ai/config.json")
    mcp = data.setdefault("mcp", {})
    mcp["devcouncil"] = _opencode_mcp_entry(project_root)
    _save_json(path, data)
    return path

def _configure_opencode(project_root: Path, apply: bool) -> bool:
    path = _opencode_config_path(project_root)
    config = {
        "$schema": "https://opencode.ai/config.json",
        "mcp": {"devcouncil": _opencode_mcp_entry(project_root)},
    }
    if not apply:
        console.print("[bold]OpenCode[/bold]")
        console.print(f"Project config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(
            "Direct executor command: "
            "[dim]opencode run --file .devcouncil/TASK-001-opencode-task.md "
            '"Execute the DevCouncil task described in the attached prompt file."[/dim]'
        )
        return True

    if not shutil.which("opencode"):
        console.print("[yellow]OpenCode CLI not found on PATH. Install it before using `dev run --executor opencode`.[/yellow]")
    try:
        written = _write_opencode_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_opencode_config(project_root)
    console.print(f"[green]OpenCode MCP config written:[/green] {written}")
    return True

def _opencode_plugin_source() -> Path:
    return Path(__file__).resolve().parents[2] / "integrations" / OPENCODE_HOOK_PLUGIN_NAME

def _opencode_plugin_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "integrations" / OPENCODE_HOOK_PLUGIN_NAME


def _opencode_plugin_ref_canonical() -> str:
    return f"./.devcouncil/integrations/{OPENCODE_HOOK_PLUGIN_NAME}"


def _is_opencode_plugin_ref(value: object) -> bool:
    """True when a plugin array entry points at the DevCouncil OpenCode hook plugin.

    Accepts the canonical ``./.devcouncil/integrations/…`` form plus common variants
    (no ``./`` prefix, backslashes, or any path ending in the plugin filename).
    """
    if not isinstance(value, str):
        return False
    normalized = value.replace("\\", "/").strip()
    if not normalized:
        return False
    canonical = _opencode_plugin_ref_canonical()
    if normalized == canonical or normalized == canonical[2:]:
        return True
    return normalized.endswith(f"/{OPENCODE_HOOK_PLUGIN_NAME}") or normalized == OPENCODE_HOOK_PLUGIN_NAME


def _opencode_plugin_registered(data: dict) -> bool:
    plugins = data.get("plugin")
    if isinstance(plugins, list):
        return any(_is_opencode_plugin_ref(item) for item in plugins)
    if isinstance(plugins, str):
        return _is_opencode_plugin_ref(plugins)
    return False


def _uninstall_opencode(project_root: Path) -> list[str]:
    """Remove DevCouncil MCP entry, plugin array ref, and generated plugin file."""
    from devcouncil.integrations.clients import hooks as _hooks

    removed: list[str] = []
    root = project_root.expanduser().resolve()
    path = _opencode_config_path(root)
    if path.exists():
        data = _load_json(path)
        changed = False
        mcp = data.get("mcp")
        if isinstance(mcp, dict) and "devcouncil" in mcp:
            mcp.pop("devcouncil")
            if not mcp:
                data.pop("mcp", None)
            changed = True
            removed.append(f"mcp.devcouncil in {path.name}")
        if changed:
            if data and set(data.keys()) != {"$schema"}:
                _save_json(path, data)
            elif data.get("$schema") and len(data) == 1:
                # Leave schema-only file; callers may still want the project file.
                _save_json(path, data)
            elif not data:
                path.unlink()
    # Plugin file + plugin[] registration (path variants included).
    removed.extend(_hooks._uninstall_opencode_hooks(root))
    removed.extend(_common._clear_client_integration_config(root, "opencode"))
    return removed
