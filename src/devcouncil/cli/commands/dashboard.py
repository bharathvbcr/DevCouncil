from pathlib import Path
import webbrowser

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
    open_browser: bool = typer.Option(False, "--open", help="Open the dashboard URL in the default browser before serving."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Serve a local live dashboard with project status, tasks, coverage, and traces."""
    if ctx.invoked_subcommand is not None:
        return
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    url = f"http://{host}:{port}"
    console.print(f"Serving DevCouncil dashboard at {url}")
    if open_browser:
        webbrowser.open(url)
    run_dashboard(root, host=host, port=port)
