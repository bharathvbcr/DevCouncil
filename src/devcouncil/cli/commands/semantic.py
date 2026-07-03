import json
import logging
import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.indexing.semantic_index import SemanticIndex
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Semantic snapshots and diffs.")
console = Console()
logger = logging.getLogger(__name__)


@app.command("snapshot")
def snapshot(
    task_id: str = typer.Argument(...),
    stage: str = typer.Option("before", "--stage", help="before or after"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_format: bool = typer.Option(False, "--json"),
):
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev semantic snapshot: task=%s stage=%s", task_id, stage)
    initialize_project(root, quiet=True)
    if stage not in {"before", "after"}:
        console.print("[red]--stage must be before or after[/red]")
        raise typer.Exit(code=2)
    with log_stage("semantic", project_root=root, subcommand="snapshot", task_id=task_id):
        log_step("semantic/1: creating snapshot", project_root=root, task_id=task_id, trace=True)
        path = SemanticIndex(root).create_snapshot(task_id, stage)
        payload = {"task_id": task_id, "stage": stage, "path": str(path)}
        if json_format:
            typer.echo(json.dumps(payload, indent=2))
        else:
            console.print(f"[green]Wrote semantic snapshot:[/green] {path}")
        log_step("semantic/complete", project_root=root, task_id=task_id, trace=True)


@app.command("diff")
def semantic_diff(
    task_id: str = typer.Argument(...),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_format: bool = typer.Option(False, "--json"),
):
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev semantic diff: task=%s", task_id)
    initialize_project(root, quiet=True)
    with log_stage("semantic", project_root=root, subcommand="diff", task_id=task_id):
        log_step("semantic/1: computing semantic diff", project_root=root, task_id=task_id, trace=True)
        result = SemanticIndex(root).diff(task_id)
        if json_format:
            typer.echo(json.dumps(result, indent=2))
        else:
            console.print(f"[cyan]Summary:[/cyan] {result['summary']}")
            for item in result["classifications"]:
                console.print(f" - {item['type']}: {item.get('path', '')}")
        log_step("semantic/complete", project_root=root, task_id=task_id, trace=True)
