import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.execution.handoff import HandoffService
from devcouncil.executors.agent_registry import load_cli_agent_specs, normalize_agent_name

console = Console()


def handoff(
    task_id: str = typer.Argument(...),
    from_agent: str = typer.Option(..., "--from"),
    to_agent: str = typer.Option(..., "--to"),
    instruction: str = typer.Option("", "--instruction"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
):
    """
    Hand off a task between coding CLI agents.
    """
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    specs = load_cli_agent_specs(root)
    from_name = normalize_agent_name(from_agent)
    to_name = normalize_agent_name(to_agent)
    if from_name not in specs or to_name not in specs:
        console.print("[red]Unknown agent name. Use dev agents list.[/red]")
        raise typer.Exit(code=1)

    try:
        manifest, path, run_id = HandoffService(root).create(
            task_id,
            from_name,
            to_name,
            instruction=instruction,
        )
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1)

    console.print(f"[green]Handoff manifest:[/green] {path}")
    console.print(f"[cyan]Next:[/cyan] dev run {task_id} --executor {to_name}")
    console.print(f"[dim]Run artifacts: .devcouncil/runs/{run_id}[/dim]")
    if instruction:
        console.print(f"[dim]Instruction: {instruction}[/dim]")
    _ = manifest
