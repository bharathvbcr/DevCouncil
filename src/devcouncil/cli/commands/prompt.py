import json
import logging
from typing import NoReturn
import typer
from rich.console import Console
from rich.markdown import Markdown
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository
from pathlib import Path

from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

@app.callback(invoke_without_command=True)
def prompt(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to generate a prompt for"),
    pretty: bool = typer.Option(False, "--pretty", help="Render the prompt for terminal reading."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON: {ok, task_id, prompt}."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Generate a constrained prompt for a specific task.
    """
    if ctx.invoked_subcommand is not None:
        return

    def _fail(message: str) -> NoReturn:
        if json_format:
            typer.echo(json.dumps({"ok": False, "task_id": task_id, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev prompt: task=%s pretty=%s", task_id, pretty)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        _fail("DevCouncil state is unavailable in this directory.")

    with log_stage("prompt", project_root=root, task_id=task_id):
        log_step("prompt/1: loading task and building prompt", project_root=root, task_id=task_id, trace=True)
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            req_repo = RequirementRepository(session)

            task = task_repo.get_by_id(task_id)
            if not task:
                _fail(f"Task {task_id} not found.")

            reqs = req_repo.get_all()

            builder = PromptBuilder(root)
            task_prompt = builder.build_task_prompt(task, reqs)

            if json_format:
                typer.echo(json.dumps({"ok": True, "task_id": task_id, "prompt": task_prompt}, indent=2))
            elif pretty:
                console.print(Markdown(task_prompt))
            else:
                typer.echo(task_prompt, nl=not task_prompt.endswith("\n"))
        log_step("prompt/complete", project_root=root, task_id=task_id, trace=True)
