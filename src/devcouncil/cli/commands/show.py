import typer
from rich.console import Console
from rich.panel import Panel
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def show(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to show"),
):
    """
    Show details of a specific task.
    """
    if ctx.invoked_subcommand is not None:
        return

    db = get_db()
    if not db:
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        raise typer.Exit(code=1)

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        req_repo = RequirementRepository(session)
        
        task = task_repo.get_by_id(task_id)
        if not task:
            console.print(f"[red]Task {task_id} not found.[/red]")
            raise typer.Exit(code=1)
        
        reqs = req_repo.get_all()
        req_map = {r.id: r for r in reqs}

        output = f"[bold]Status:[/bold] {task.status}\n\n"
        output += f"[bold]Description:[/bold]\n{task.description}\n\n"
        
        output += "[bold]Linked Requirements:[/bold]\n"
        for req_id in task.requirement_ids:
            req = req_map.get(req_id)
            if req:
                output += f"  - [cyan]{req.id}[/cyan]: {req.title}\n"
            else:
                output += f"  - [cyan]{req_id}[/cyan]: (Requirement not found)\n"
                
        output += "\n[bold]Planned Files:[/bold]\n"
        for pf in task.planned_files:
            output += f"  - {pf.path} ({pf.allowed_change}): {pf.reason}\n"
            
        output += "\n[bold]Expected Tests:[/bold]\n"
        for et in task.expected_tests:
            output += f"  - {et}\n"

        console.print(Panel(output, title=f"Task {task.id}: {task.title}", expand=False))
