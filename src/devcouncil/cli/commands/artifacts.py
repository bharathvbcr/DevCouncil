import typer
from rich.console import Console
from rich.table import Table
from pathlib import Path

from devcouncil.app.errors import GatingError
from devcouncil.artifacts.validators import ArtifactValidator
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import RequirementRepository, TaskRepository
from devcouncil.cli.commands.init import initialize_project

app = typer.Typer()
console = Console()


@app.command(name="validate")
def validate(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Validate requirements and tasks stored in the artifact graph."""
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    errors: list[str] = []
    with db.get_session() as session:
        req_repo = RequirementRepository(session)
        task_repo = TaskRepository(session)

        for req in req_repo.get_all():
            try:
                ArtifactValidator.validate_requirement(req)
            except GatingError as exc:
                errors.append(str(exc))

        for task in task_repo.get_all():
            try:
                ArtifactValidator.validate_task(task)
            except GatingError as exc:
                errors.append(str(exc))

    if not errors:
        console.print("[green]Artifacts are valid.[/green]")
        return

    table = Table(title="Artifact Validation Errors")
    table.add_column("Error", style="red")
    for error in errors:
        table.add_row(error)
    console.print(table)
    raise typer.Exit(code=1)
