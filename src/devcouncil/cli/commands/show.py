from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path

import typer
from rich.console import Console
from rich.panel import Panel
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

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
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev show: task=%s json=%s", task_id, json_format)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        raise typer.Exit(code=1)

    with log_stage("show", project_root=root, task_id=task_id):
        log_step("show/1: loading task details", project_root=root, task_id=task_id, trace=True)
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            req_repo = RequirementRepository(session)

            task = task_repo.get_by_id(task_id)
            if not task:
                message = f"Task {task_id} not found."
                if json_format:
                    typer.echo(dump_json({"ok": False, "error": message, "task_id": task_id}, indent=2))
                else:
                    console.print(f"[red]{message}[/red]")
                raise typer.Exit(code=1)

            reqs = req_repo.get_all()
            req_map = {r.id: r for r in reqs}

            if json_format:
                linked_requirements = [
                    req_map[req_id].model_dump()
                    for req_id in task.requirement_ids
                    if req_id in req_map
                ]
                typer.echo(dump_json({
                    "task": task.model_dump(),
                    "linked_requirements": linked_requirements,
                }, indent=2))
                log_step("show/complete", project_root=root, task_id=task_id, trace=True)
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
        log_step("show/complete", project_root=root, task_id=task_id, trace=True)
