import typer
from rich.console import Console
from sqlmodel import delete
from pathlib import Path

from devcouncil.storage.db import get_db
from devcouncil.storage.models import EvidenceModel, GapModel, RequirementModel, TaskModel
from devcouncil.cli.commands.init import initialize_project

console = Console()


def reset_demo_state(
    yes: bool = typer.Option(False, "--yes", help="Confirm clearing planning/demo artifacts."),
):
    """Clear demo planning artifacts from the local DevCouncil state database."""
    if not yes:
        console.print("[red]Refusing to clear state without --yes.[/red]")
        raise typer.Exit(code=1)

    initialize_project(Path("."), quiet=True)
    db = get_db()
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    with db.get_session() as session:
        for model in (EvidenceModel, GapModel, TaskModel, RequirementModel):
            session.exec(delete(model))

    console.print("[green]Cleared requirements, tasks, gaps, and evidence from local state.[/green]")
