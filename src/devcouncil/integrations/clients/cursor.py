"""Cursor integration adapter."""
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
def _cursor_config_path(project_root: Path) -> Path:
    return project_root / ".cursor" / "mcp.json"

def _cursor_mcp_config(project_root: Path) -> dict:
    return {
        "mcpServers": {
            "devcouncil": {
                "type": "stdio",
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
            }
        }
    }

def _write_cursor_config(project_root: Path) -> Path:
    path = _cursor_config_path(project_root)
    data = _load_json_strict(path, "Cursor")
    mcp_servers = data.setdefault("mcpServers", {})
    mcp_servers["devcouncil"] = _cursor_mcp_config(project_root)["mcpServers"]["devcouncil"]
    _save_json(path, data)
    return path

def _record_cursor_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "enabled": True,
            "config_path": str(_cursor_config_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)

def _configure_cursor(project_root: Path, apply: bool) -> bool:
    path = _cursor_config_path(project_root)
    config = _cursor_mcp_config(project_root)
    if not apply:
        console.print("[bold]Cursor[/bold]")
        console.print(f"Project MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print("Verify in Cursor CLI with: [dim]cursor-agent mcp list[/dim]")
        return True

    if not shutil.which("cursor") and not shutil.which("cursor-agent"):
        console.print("[yellow]Cursor CLI not found on PATH. Project MCP config will still be available to Cursor.[/yellow]")
    try:
        written = _write_cursor_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_cursor_config(project_root)
    console.print(f"[green]Cursor MCP config written:[/green] {written}")
    return True
