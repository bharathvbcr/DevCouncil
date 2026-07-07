"""Antigravity integration adapter."""
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
def _antigravity_mcp_path(project_root: Path) -> Path:
    return project_root / ".agents" / "mcp_config.json"

def _antigravity_mcp_config(project_root: Path) -> dict:
    return {
        "mcpServers": {
            "devcouncil": {
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
                "cwd": str(project_root),
            }
        }
    }

def _record_antigravity_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        antigravity = config.setdefault("integrations", {}).setdefault("antigravity", {})
        antigravity.update({
            "enabled": True,
            "mcp_config_path": str(_antigravity_mcp_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)

def _write_antigravity_mcp_config(project_root: Path) -> Path:
    path = _antigravity_mcp_path(project_root)
    data = _load_json_strict(path, "Antigravity")
    mcp_servers = data.setdefault("mcpServers", {})
    mcp_servers["devcouncil"] = _antigravity_mcp_config(project_root)["mcpServers"]["devcouncil"]
    _save_json(path, data)
    return path

def _configure_antigravity(project_root: Path, apply: bool) -> bool:
    path = _antigravity_mcp_path(project_root)
    config = _antigravity_mcp_config(project_root)
    if not apply:
        console.print("[bold]Google Antigravity CLI[/bold]")
        console.print(f"Project MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(
            "Direct executor command: "
            "[dim]agy --print --print-timeout 30m "
            '"Read and execute the DevCouncil task prompt at .devcouncil/TASK-001-antigravity-task.md."[/dim]'
        )
        return True

    if not shutil.which("agy"):
        console.print("[yellow]Antigravity CLI (`agy`) not found on PATH. Install it before using `dev run --executor antigravity`.[/yellow]")
    try:
        written = _write_antigravity_mcp_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_antigravity_config(project_root)
    console.print(f"[green]Antigravity MCP config written:[/green] {written}")
    return True
