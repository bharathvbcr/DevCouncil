import json
import logging
from pathlib import Path

import typer

from devcouncil.indexing.lsp import LspInspector
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Inspect optional LSP integration readiness.")
logger = logging.getLogger(__name__)


@app.command("inspect")
def inspect(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Print detected language servers and initialize payloads."""
    root = project_root.expanduser().resolve()
    # Existence check FIRST: set_log_dir/log_stage create .devcouncil/ under the
    # root, which would materialize a missing/typo'd path and turn the "does not
    # exist" report into a false success.
    if not root.exists():
        typer.echo(json.dumps({"languages": [], "servers": [], "initialize_requests": {}, "error": f"{root} does not exist"}, indent=2))
        raise typer.Exit(code=1)
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev lsp inspect")
    with log_stage("lsp", project_root=root):
        log_step("lsp/1: inspecting language servers", project_root=root, trace=True)
        typer.echo(LspInspector(root).summary_json())
        log_step("lsp/complete", project_root=root, trace=True)
