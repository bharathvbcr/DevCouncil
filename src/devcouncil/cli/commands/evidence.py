import json
import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.verification.test_resolver import TestResolver
from devcouncil.verification.verifier import Verifier

app = typer.Typer(help="Evidence suggestion utilities.")
console = Console()


@app.command("suggest")
def suggest(
    task_id: str = typer.Argument(...),
    apply: bool = typer.Option(False, "--apply"),
    include_low_confidence: bool = typer.Option(False, "--include-low-confidence"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
):
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
        changed = Verifier(root).get_task_changed_files(task_id)
        suggestions = TestResolver(root).suggest_for_task(task, changed)
        if not include_low_confidence:
            suggestions = [s for s in suggestions if s.confidence != "low"]
        if apply:
            for item in suggestions:
                if item.confidence == "high" and item.command not in task.expected_tests:
                    task.expected_tests.append(item.command)
            TaskRepository(session).save(task)
        typer.echo(json.dumps({
            "task_id": task_id,
            "suggestions": [s.model_dump() for s in suggestions],
            "expected_tests": task.expected_tests,
        }, indent=2))
