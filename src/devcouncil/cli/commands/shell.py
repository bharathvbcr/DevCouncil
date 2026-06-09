import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.execution.shell_session import GuardedShellSession
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository

console = Console()


def shell(
    task_id: str = typer.Argument(..., help="Task ID"),
    command: str | None = typer.Option(None, "--command", help="Run one guarded command and exit."),
    shell_name: str = typer.Option("auto", "--shell", help="Shell backend: auto, pwsh, bash, zsh."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
):
    """
    Run guarded shell commands for a task.
    """
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil not initialized.[/red]")
        raise typer.Exit(code=1)

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
    session_runner.start()
    try:
        if command:
            code = session_runner.run_one(command)
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
