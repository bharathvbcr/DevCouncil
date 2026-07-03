import json
import logging
import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.execution.fs_watcher import FilesystemWatcher
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def watch_fs(
    task_id: str = typer.Option(..., "--task", help="Task ID to attribute file changes to."),
    poll_interval: float = typer.Option(1.0, "--poll-interval"),
    once: bool = typer.Option(False, "--once", help="Scan once and exit."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_format: bool = typer.Option(False, "--json"),
):
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev watch-fs: task=%s once=%s", task_id, once)
    initialize_project(root, quiet=True)

    with log_stage("watch_fs", project_root=root, task_id=task_id, once=once):
        log_step("watch_fs/1: starting filesystem watcher", project_root=root, task_id=task_id, trace=True)

        def _print_event(event: dict) -> None:
            status = "allowed" if event["allowed"] else "denied"
            console.print(f"[cyan]{event['path']}[/cyan] {status}: {event['reason']}")

        watcher = FilesystemWatcher(root, task_id, poll_interval=poll_interval, on_event=_print_event)
        if once:
            events = watcher.scan_once()
            if json_format:
                typer.echo(json.dumps({"events": events}, indent=2))
            else:
                for event in events:
                    status = "allowed" if event["allowed"] else "denied"
                    console.print(f"[cyan]{event['path']}[/cyan] {status}: {event['reason']}")
            log_step("watch_fs/complete", project_root=root, task_id=task_id, count=len(events), trace=True)
            return
        console.print(f"[cyan]Watching filesystem for task {task_id}. Ctrl+C to stop.[/cyan]")
        try:
            watcher.watch()
        except KeyboardInterrupt:
            console.print("[yellow]Stopped filesystem watcher.[/yellow]")
        log_step("watch_fs/complete", project_root=root, task_id=task_id, trace=True)
