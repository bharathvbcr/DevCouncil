"""Cursor integration adapter."""
from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

from devcouncil.executors.agent_registry import resolve_cursor_agent_executable
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


def _cursor_executable() -> str | None:
    return resolve_cursor_agent_executable()


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


def probe_cursor_auth() -> tuple[bool, str]:
    """Probe Cursor CLI auth via agent status/about."""
    executable = _cursor_executable()
    if not executable:
        return False, "agent/cursor-agent not found on PATH"
    if os.environ.get("CURSOR_API_KEY"):
        return True, "CURSOR_API_KEY set (CI headless auth)"
    for subcommand in ("status", "about"):
        code, output = _run_capture([executable, subcommand])
        if code == 0 and output.strip():
            first = output.strip().splitlines()[0]
            return True, first
    return False, "Run agent login or set CURSOR_API_KEY for CI headless auth"


def probe_cursor_mcp_list() -> tuple[bool, str]:
    executable = _cursor_executable()
    if not executable:
        return False, "agent/cursor-agent not found on PATH"
    code, output = _run_capture([executable, "mcp", "list"])
    if code != 0:
        return False, output.strip() or "agent mcp list failed"
    return True, output.strip().splitlines()[0] if output.strip() else "MCP servers listed"


def _configure_cursor(project_root: Path, apply: bool) -> bool:
    path = _cursor_config_path(project_root)
    config = _cursor_mcp_config(project_root)
    executable = _cursor_executable() or "agent"
    if not apply:
        console.print("[bold]Cursor[/bold]")
        console.print(f"Project MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(f"Verify in Cursor CLI with: [dim]{executable} mcp list[/dim]")
        auth_ok, auth_details = probe_cursor_auth()
        if auth_ok:
            console.print(f"Auth: [green]{auth_details}[/green]")
        else:
            console.print(f"Auth: [yellow]{auth_details}[/yellow]")
        if not os.environ.get("CURSOR_API_KEY"):
            console.print("CI headless: export [dim]CURSOR_API_KEY[/dim] (see Cursor headless docs)")
        return True

    if not shutil.which("cursor") and not _cursor_executable():
        console.print("[yellow]Cursor CLI not found on PATH. Project MCP config will still be available to Cursor.[/yellow]")
    try:
        written = _write_cursor_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_cursor_config(project_root)
    console.print(f"[green]Cursor MCP config written:[/green] {written}")
    mcp_ok, mcp_details = probe_cursor_mcp_list()
    if mcp_ok:
        console.print(f"[dim]MCP probe: {mcp_details}[/dim]")
    auth_ok, auth_details = probe_cursor_auth()
    if not auth_ok:
        console.print(f"[yellow]Auth check: {auth_details}[/yellow]")
    return True
