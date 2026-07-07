"""Aider integration adapter."""
from __future__ import annotations

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
def _record_aider_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        config.setdefault("integrations", {}).setdefault("aider", {}).update({"enabled": True})

    _mutate_raw_config(project_root, mutate)


def _configure_aider(project_root: Path, apply: bool) -> bool:
    command = ["aider", "--yes", "--no-show-model-warnings", "--message", "<task prompt>"]
    if not apply:
        console.print("[bold]Aider[/bold]")
        console.print("Built-in executor: [dim]dev run TASK-001 --executor aider[/dim]")
        console.print("Launch command: [dim]" + _format_command(command) + "[/dim]")
        console.print("Aider does not expose a first-party DevCouncil MCP server.")
        return True

    if not shutil.which("aider"):
        console.print("[yellow]Aider CLI not found on PATH. Install it before using `dev run --executor aider`.[/yellow]")
    _record_aider_config(project_root)
    console.print("[green]Aider executor enabled in .devcouncil/config.yaml.[/green]")
    return True


