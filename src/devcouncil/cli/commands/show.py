import json
from pathlib import Path

import typer
from rich.console import Console
from rich.panel import Panel
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def show(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to show"),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Show details of a specific task.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
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

        if json_format:
            linked_requirements = [
                req_map[req_id].model_dump()
                for req_id in task.requirement_ids
                if req_id in req_map
            ]
            typer.echo(json.dumps({
                "task": task.model_dump(),
                "linked_requirements": linked_requirements,
            }, indent=2))
            return

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
