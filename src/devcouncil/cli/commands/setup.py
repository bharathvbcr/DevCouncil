from pathlib import Path
import shutil

import typer
from rich.console import Console
from rich.panel import Panel

from devcouncil.cli.commands.doctor import render_doctor_check
from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.commands.integrate import _codex_command, _configure, _gemini_command

app = typer.Typer()
console = Console()


@app.callback(invoke_without_command=True)
def setup(
    ctx: typer.Context,
    project_root: Path = typer.Option(
        Path("."),
        "--project-root",
        help="Target project repository root. Defaults to the terminal's current directory.",
    ),
    name: str | None = typer.Option(None, "--name", "-n", help="Project name for .devcouncil/config.yaml."),
    integrate: bool = typer.Option(False, "--integrate", help="Configure supported coding CLI MCP integrations."),
    apply: bool = typer.Option(False, "--apply", help="Apply integration config instead of previewing commands."),
    gemini_scope: str = typer.Option("project", "--gemini-scope", help="Gemini MCP config scope: project or user."),
):
    """
    Initialize DevCouncil from a normal terminal in the target repository root.

    Use the coding CLI later only for the generated dev prompt output.
    """
    if ctx.invoked_subcommand is not None:
        return

    if gemini_scope not in {"project", "user"}:
        console.print("[red]--gemini-scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = project_root.expanduser().resolve()
    created = initialize_project(root, project_name=name)
    if not created:
        console.print(f"[yellow]DevCouncil is already initialized at {root / '.devcouncil'}.[/yellow]")

    console.print()
    render_doctor_check()

    if integrate:
        console.print()
        console.print("[bold]Coding CLI integration[/bold]")
        commands = [
            ("Codex CLI", _codex_command(root)),
            ("Gemini CLI", _gemini_command(root, gemini_scope)),
        ]
        results = []
        for tool, command in commands:
            if apply and not shutil.which(command[0]):
                console.print(f"[yellow]{tool} CLI not found on PATH. Skipping optional integration.[/yellow]")
                continue
            results.append(_configure(tool, command, apply))
        if apply and any(not ok for ok in results):
            raise typer.Exit(code=1)

    console.print()
    console.print(Panel.fit(
        "\n".join([
            "[bold]Next commands[/bold]",
            f"Keep running DevCouncil commands in this terminal at: {root}",
            "dev plan \"Describe the implementation goal\"",
            "dev tasks",
            "dev run TASK-001 --executor manual",
            "dev prompt TASK-001",
            "Paste only the dev prompt output into your coding CLI.",
            "dev verify TASK-001",
            "",
            "Use [bold]dev setup --integrate[/bold] to preview coding CLI MCP setup.",
            "Use [bold]dev setup --integrate --apply[/bold] to configure detected clients.",
        ]),
        title="DevCouncil is ready",
        border_style="green",
    ))
