"""`dev logs` — find and read DevCouncil's runtime logs.

Comprehensive logging only helps if the logs are easy to reach when something
breaks. This command surfaces the always-on shared log
(``.devcouncil/logs/devcouncil.log``) and the per-run logs
(``.devcouncil/runs/<run_id>/run.log``) without the user needing to remember
paths or hand-roll ``tail``/``grep``.

DELIBERATELY UNINSTRUMENTED: this command's subject *is* the log. Emitting log
records (``logger.info`` / ``log_stage`` / ``log_step``) from here mutates what
the user asked to inspect — ``tail`` displaces the trailing lines it was asked
to show with its own records, and ``path`` creates the very file it is supposed
to report as "not created yet". An introspection command must not modify what
it introspects, so no stage instrumentation belongs in this file.
"""

import time
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console

from devcouncil.telemetry.logging_setup import LOG_RELATIVE_PATH

app = typer.Typer(help="View DevCouncil runtime logs (shared and per-run).")
console = Console()


def _shared_log(root: Path) -> Path:
    return root / LOG_RELATIVE_PATH


def _runs_dir(root: Path) -> Path:
    return root / ".devcouncil" / "runs"


def _print_tail(path: Path, limit: int, grep: Optional[str]) -> None:
    if not path.exists():
        console.print(f"[yellow]No log at {path}. Run a command first (or pass --project-root).[/yellow]")
        raise typer.Exit(code=0)
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    if grep:
        lines = [line for line in lines if grep.lower() in line.lower()]
    for line in lines[-limit:]:
        console.print(line, markup=False, highlight=False)


@app.command("tail")
def tail(
    limit: int = typer.Option(50, "--limit", "-n", help="Number of trailing lines to show."),
    follow: bool = typer.Option(False, "--follow", "-f", help="Keep printing new lines as they are written."),
    grep: Optional[str] = typer.Option(None, "--grep", "-g", help="Only show lines containing this substring (case-insensitive)."),
    run: Optional[str] = typer.Option(None, "--run", help="Show a specific run's log (.devcouncil/runs/<run>/run.log) instead of the shared log."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Print the tail of the shared log (or a per-run log with --run)."""
    root = project_root.expanduser().resolve()
    path = (_runs_dir(root) / run / "run.log") if run else _shared_log(root)

    _print_tail(path, limit, grep)
    if not follow:
        return

    console.print(f"[dim]— following {path} (Ctrl-C to stop) —[/dim]")
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            handle.seek(0, 2)
            while True:
                line = handle.readline()
                if line:
                    rendered = line.rstrip("\n")
                    if not grep or grep.lower() in rendered.lower():
                        console.print(rendered, markup=False, highlight=False)
                else:
                    time.sleep(0.4)
    except KeyboardInterrupt:  # pragma: no cover - interactive
        pass


@app.command("path")
def path(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Print the shared log file path (and whether it exists)."""
    root = project_root.expanduser().resolve()
    log = _shared_log(root)
    marker = "" if log.exists() else " [yellow](not created yet)[/yellow]"
    console.print(f"{log}{marker}")


@app.command("runs")
def runs(
    limit: int = typer.Option(20, "--limit", "-n", help="Maximum number of recent run logs to list."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """List per-run logs, newest first, with their paths."""
    root = project_root.expanduser().resolve()
    runs_dir = _runs_dir(root)
    if not runs_dir.exists():
        console.print("[yellow]No runs yet.[/yellow]")
        return
    run_logs = [d / "run.log" for d in runs_dir.iterdir() if (d / "run.log").exists()]
    if not run_logs:
        console.print("[yellow]No per-run logs yet (run.log appears once an executor runs).[/yellow]")
        return
    run_logs.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    for log in run_logs[:limit]:
        size_kb = log.stat().st_size / 1024
        console.print(f"[bold]{log.parent.name}[/bold]  [dim]{log}  ({size_kb:.1f} KB)[/dim]")
    console.print("\n[dim]View one with:[/dim] dev logs tail --run <run-id>")
