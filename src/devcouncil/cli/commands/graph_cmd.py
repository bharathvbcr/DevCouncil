from pathlib import Path

import typer

from devcouncil.indexing.viz import write_graph_demo

app = typer.Typer(help="Interactive code-graph HTML visualizer.")


@app.command("demo")
def graph_demo(
    open_browser: bool = typer.Option(True, "--open/--no-open", help="Open demo.html in the default browser."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Open a sample graph visualizer (real UI + synthetic graph; no ``dev map`` required)."""
    root = project_root.expanduser().resolve()
    out = write_graph_demo(root, open_browser=open_browser)
    typer.echo(out)
