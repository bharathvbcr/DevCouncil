import json
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def tasks(
    ctx: typer.Context,
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    List task graph and task gate status.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        tasks_list = task_repo.get_all()

        if json_format:
            typer.echo(json.dumps({"tasks": [task.model_dump() for task in tasks_list]}, indent=2))
            return
        
        if not tasks_list:
            console.print("No tasks found. Run 'dev plan' to generate tasks.")
            return

        table = Table(title="DevCouncil Tasks")
        table.add_column("Task ID", style="cyan", no_wrap=True)
        table.add_column("Title", style="white")
        table.add_column("Status", style="magenta")
        table.add_column("Linked Reqs", style="green")

        for t in tasks_list:
            reqs = ", ".join(t.requirement_ids)
            table.add_row(t.id, t.title, t.status, reqs)

        console.print(table)
