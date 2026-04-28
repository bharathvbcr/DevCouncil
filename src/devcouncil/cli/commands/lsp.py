import json
from pathlib import Path

import typer

from devcouncil.indexing.lsp import LspInspector

app = typer.Typer(help="Inspect optional LSP integration readiness.")


@app.command("inspect")
def inspect(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Print detected language servers and initialize payloads."""
    root = project_root.expanduser().resolve()
    if not root.exists():
        typer.echo(json.dumps({"languages": [], "servers": [], "initialize_requests": {}, "error": f"{root} does not exist"}, indent=2))
        raise typer.Exit(code=1)
    typer.echo(LspInspector(root).summary_json())
