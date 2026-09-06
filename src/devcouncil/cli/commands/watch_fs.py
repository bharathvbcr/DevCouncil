from devcouncil.utils.json_persist import dump_json
import logging
import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.execution.fs_watcher import FilesystemWatcher
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
# Diagnostics go to stderr unconditionally — same split as `dev map`/`dev graph`/`dev debug`.
# The watcher's live event feed is one of them: it fires from inside `scan_once()`, so
# under `--once --json` it was writing a line per changed file in front of the payload.
status_console = Console(stderr=True)
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

        def _format_event(event: dict) -> str:
            status = "allowed" if event["allowed"] else "denied"
            return f"[cyan]{event['path']}[/cyan] {status}: {event['reason']}"

        # Live progress as the watcher walks the tree, distinct from the `--once` result
        # below: this fires per event during the scan, so it belongs on stderr in both
        # modes. In `--once` human mode the same events are then rendered to stdout as
        # the command's actual output.
        watcher = FilesystemWatcher(
            root,
            task_id,
            poll_interval=poll_interval,
            on_event=lambda event: status_console.print(_format_event(event)),
        )
        if once:
            events = watcher.scan_once()
            if json_format:
                typer.echo(dump_json({"events": events}, indent=2))
            else:
                for event in events:
                    console.print(_format_event(event))
            log_step("watch_fs/complete", project_root=root, task_id=task_id, count=len(events), trace=True)
            return
        # Follow mode never reaches a payload — it runs until interrupted — so under
        # `--json` stdout must stay empty rather than collect these two banners.
        status_console.print(f"[cyan]Watching filesystem for task {task_id}. Ctrl+C to stop.[/cyan]")
        try:
            watcher.watch()
        except KeyboardInterrupt:
            status_console.print("[yellow]Stopped filesystem watcher.[/yellow]")
        log_step("watch_fs/complete", project_root=root, task_id=task_id, trace=True)
