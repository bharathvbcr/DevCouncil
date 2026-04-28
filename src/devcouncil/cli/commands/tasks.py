import typer
from rich.console import Console
from rich.table import Table
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def tasks(ctx: typer.Context):
    """
    List task graph and task gate status.
    """
    if ctx.invoked_subcommand is not None:
        return

    db = get_db()
    if not db:
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        raise typer.Exit(code=1)

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        tasks_list = task_repo.get_all()
        
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
