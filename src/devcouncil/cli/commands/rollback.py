import logging
import typer
from pathlib import Path
from rich.console import Console

from devcouncil.execution.checkpoints import CheckpointService
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

@app.callback(invoke_without_command=True)
def rollback(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to rollback"),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Revert changes using a task's git checkpoint.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev rollback: task=%s", task_id)

    with log_stage("rollback", project_root=root, task_id=task_id):
        log_step("rollback/1: locating checkpoint", project_root=root, task_id=task_id, trace=True)
        checkpoint_dir = root / ".devcouncil" / "checkpoints"
        checkpoint_file = checkpoint_dir / f"{task_id}-before.patch"
        after_patch = checkpoint_dir / f"{task_id}-after.patch"
        service = CheckpointService(root)

        if not checkpoint_file.exists() and not after_patch.exists():
            before_ref = CheckpointService.REF_BEFORE.format(task_id=task_id)
            after_ref = CheckpointService.REF_AFTER.format(task_id=task_id)
            if not service._ref_exists(before_ref) and not service._ref_exists(after_ref):
                console.print(
                    f"[red]No checkpoint found for task {task_id}. Expected {after_patch} "
                    f"or {checkpoint_file}.[/red]"
                )
                raise typer.Exit(code=1)

        console.print(f"Rolling back task [bold]{task_id}[/bold]...")
        log_step("rollback/2: reverting checkpoint", project_root=root, task_id=task_id)
        result = service.rollback(task_id)
        if "failed" in result.message.lower() or result.message.startswith("No checkpoint"):
            console.print(f"[yellow]{result.message}[/yellow]")
            if checkpoint_file.exists():
                console.print(
                    f"The before-patch at {checkpoint_file} captured the state before the task ran.\n"
                    f"To manually reset:\n"
                    f"  1. [bold]git stash[/bold] (if you want to keep current changes)\n"
                    f"  2. [bold]git checkout -- .[/bold] (discard working tree changes)\n"
                    f"  3. [bold]git apply {checkpoint_file}[/bold] (restore pre-task state)"
                )
            elif after_patch.exists():
                console.print(
                    f"Only the after-patch at {after_patch} exists (it captured the task's changes).\n"
                    f"To manually revert those changes from the working tree:\n"
                    f"  1. [bold]git apply --stat {after_patch}[/bold] (inspect what the task changed)\n"
                    f"  2. [bold]git apply -R {after_patch}[/bold] (reverse-apply the task's changes)"
                )
            raise typer.Exit(code=1)

        console.print(f"[green]Successfully rolled back task {task_id}.[/green] {result.message}")
        log_step("rollback/complete", project_root=root, task_id=task_id, trace=True)
