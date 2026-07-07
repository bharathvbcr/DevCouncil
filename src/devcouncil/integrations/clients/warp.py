"""Warp integration adapter."""
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
def _warp_mcp_config(project_root: Path) -> dict:
    return {
        "devcouncil": {
            "command": "devcouncil",
            "args": ["mcp-server"],
            "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
        }
    }

def _warp_mcp_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "integrations" / "warp-mcp.json"

def _write_warp_mcp_config(project_root: Path) -> Path:
    path = _warp_mcp_path(project_root)
    _save_json(path, _warp_mcp_config(project_root))
    return path

def _record_warp_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        warp = config.setdefault("integrations", {}).setdefault("warp", {})
        warp.update({
            "enabled": True,
            "command": warp.get("command", "oz"),
            "run_mode": warp.get("run_mode", "local"),
            "mcp_config_path": str(_warp_mcp_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)

def _configure_warp(project_root: Path, apply: bool) -> bool:
    path = _warp_mcp_path(project_root)
    config = _warp_mcp_config(project_root)
    if not apply:
        console.print("[bold]Warp / Oz[/bold]")
        console.print(f"MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(f"Direct executor command: [dim]oz agent run --cwd {project_root} --mcp {path} --prompt <task prompt>[/dim]")
        return True

    written = _write_warp_mcp_config(project_root)
    _record_warp_config(project_root)
    console.print(f"[green]Warp MCP config written:[/green] {written}")
    if not shutil.which("oz"):
        console.print("[yellow]oz CLI not found on PATH. Install Warp/Oz before using `dev run --executor warp`.[/yellow]")
    return True
