"""Grok Build integration adapter."""
from __future__ import annotations

import shutil
from pathlib import Path

from devcouncil.integrations.clients import common as _common

_project_root = _common._project_root
_format_command = _common._format_command
_run = _common._run
_mutate_raw_config = _common._mutate_raw_config
_configure = _common._configure

console = _common.console


def _grok_config_path(project_root: Path) -> Path:
    return project_root / ".grok" / "config.toml"


def _grok_mcp_toml_snippet(project_root: Path) -> str:
    root = str(project_root)
    return (
        "[mcp_servers.devcouncil]\n"
        'command = "devcouncil"\n'
        'args = ["mcp-server"]\n'
        f'env = {{ DEVCOUNCIL_PROJECT_ROOT = "{root}" }}\n'
    )


def _grok_mcp_command(project_root: Path) -> list[str]:
    return [
        "grok",
        "mcp",
        "add",
        "devcouncil",
        "--scope",
        "project",
        "-e",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "--",
        "devcouncil",
        "mcp-server",
    ]


def _merge_grok_config_toml(project_root: Path) -> Path:
    path = _grok_config_path(project_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    snippet = _grok_mcp_toml_snippet(project_root)
    existing = path.read_text(encoding="utf-8") if path.exists() else ""
    if "mcp_servers.devcouncil" in existing or "[mcp_servers.devcouncil]" in existing:
        return path
    separator = "\n" if existing and not existing.endswith("\n") else ""
    path.write_text(f"{existing}{separator}\n{snippet}", encoding="utf-8")
    return path


def _record_grok_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        grok = config.setdefault("integrations", {}).setdefault("grok", {})
        grok.update({
            "enabled": True,
            "config_path": str(_grok_config_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)


def _configure_grok(project_root: Path, apply: bool) -> bool:
    path = _grok_config_path(project_root)
    cli_command = _grok_mcp_command(project_root)
    if not apply:
        console.print("[bold]Grok Build[/bold]")
        console.print(f"Preferred MCP registration: [dim]{_format_command(cli_command)}[/dim]")
        console.print(f"Fallback project MCP config: [dim]{path}[/dim]")
        console.print(_grok_mcp_toml_snippet(project_root), soft_wrap=True)
        console.print(
            "Project hooks require trust — run [dim]/hooks-trust[/dim] in Grok after install."
        )
        console.print("Verify with: [dim]grok mcp list --json[/dim]")
        return True

    if shutil.which("grok"):
        code = _run(cli_command)
        if code == 0:
            _record_grok_config(project_root)
            console.print("[green]Grok MCP server registered via grok mcp add.[/green]")
            return True
        console.print(
            "[yellow]grok mcp add failed; falling back to .grok/config.toml merge.[/yellow]"
        )

    try:
        written = _merge_grok_config_toml(project_root)
    except OSError as exc:
        console.print(f"[red]Failed to write Grok MCP config: {exc}[/red]")
        return False
    _record_grok_config(project_root)
    console.print(f"[green]Grok MCP config written:[/green] {written}")
    console.print(
        "[dim]Trust project hooks with /hooks-trust (or grok --trust) before pre-action gates run.[/dim]"
    )
    return True
