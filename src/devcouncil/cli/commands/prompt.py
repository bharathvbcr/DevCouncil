import typer
from rich.console import Console
from rich.markdown import Markdown
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository
from devcouncil.execution.prompt_builder import PromptBuilder

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def prompt(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to generate a prompt for"),
):
    """
    Generate a constrained prompt for a specific task.
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
        
        builder = PromptBuilder()
        task_prompt = builder.build_task_prompt(task, reqs)
        
        console.print(Markdown(task_prompt))
