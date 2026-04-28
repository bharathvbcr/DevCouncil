from pathlib import Path

import typer
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.ui.dashboard import run_dashboard

app = typer.Typer(help="Serve the live DevCouncil dashboard.")
console = Console()


@app.callback(invoke_without_command=True)
def dashboard(
    ctx: typer.Context,
    host: str = typer.Option("127.0.0.1", "--host", help="Dashboard bind host."),
    port: int = typer.Option(8765, "--port", help="Dashboard bind port."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Serve a local live dashboard with project status, tasks, coverage, and traces."""
    if ctx.invoked_subcommand is not None:
        return
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    console.print(f"Serving DevCouncil dashboard at http://{host}:{port}")
    run_dashboard(root, host=host, port=port)
