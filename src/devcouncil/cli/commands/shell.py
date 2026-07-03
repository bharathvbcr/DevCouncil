import logging
import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.execution.shell_session import GuardedShellSession
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def shell(
    task_id: str = typer.Argument(..., help="Task ID"),
    command: str | None = typer.Option(None, "--command", help="Run one guarded command and exit."),
    shell_name: str = typer.Option("auto", "--shell", help="Shell backend: auto, pwsh, bash, zsh."),
    force: bool = typer.Option(False, "--force", help="Reclaim a stale lease from a previous (possibly crashed) session."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
):
    """
    Run guarded shell commands for a task.
    """
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev shell: task=%s command=%s", task_id, "set" if command else "interactive")
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil not initialized.[/red]")
        raise typer.Exit(code=1)

    with log_stage("shell", project_root=root, task_id=task_id):
        log_step("shell/1: loading task and starting session", project_root=root, task_id=task_id, trace=True)
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                console.print(f"[red]Task {task_id} not found.[/red]")
                raise typer.Exit(code=1)

        try:
            session_runner = GuardedShellSession(root, task, shell=shell_name)
        except ValueError as exc:
            console.print(f"[red]{exc}[/red]")
            raise typer.Exit(code=2)
        try:
            session_runner.start(force=force)
        except ValueError as exc:
            console.print(f"[red]{exc}[/red]")
            console.print(
                "[yellow]Another shell session may be active for this task. If it is "
                "stale (a previous session crashed), re-run with [bold]--force[/bold] to "
                "reclaim it.[/yellow]"
            )
            raise typer.Exit(code=2)
        try:
            if command:
                log_step("shell/2: running guarded command", project_root=root, task_id=task_id)
                code = session_runner.run_one(command)
                log_step("shell/complete", project_root=root, task_id=task_id, trace=True)
                raise typer.Exit(code=code)

            console.print(f"[cyan]Guarded shell for {task_id}. Type exit or quit to end.[/cyan]")
            while True:
                try:
                    line = input(f"devcouncil:{task_id}> ")
                except (EOFError, KeyboardInterrupt):
                    break
                normalized = line.strip()
                if normalized.lower() in {"exit", "quit"}:
                    break
                if not normalized:
                    continue
                code = session_runner.run_one(normalized)
                if code != 0:
                    console.print(f"[yellow]Command exited with {code}[/yellow]")
        finally:
            session_runner.finish()
        log_step("shell/complete", project_root=root, task_id=task_id, trace=True)
