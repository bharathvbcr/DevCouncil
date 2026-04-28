import typer
from rich.console import Console
from rich.markdown import Markdown
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository
from pathlib import Path

from devcouncil.execution.prompt_builder import PromptBuilder

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def prompt(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to generate a prompt for"),
    pretty: bool = typer.Option(False, "--pretty", help="Render the prompt for terminal reading."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Generate a constrained prompt for a specific task.
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
        
        builder = PromptBuilder(root)
        task_prompt = builder.build_task_prompt(task, reqs)

        if pretty:
            console.print(Markdown(task_prompt))
        else:
            typer.echo(task_prompt, nl=not task_prompt.endswith("\n"))
