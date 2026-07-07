from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path

import typer

from devcouncil.indexing.lsp import LspInspector
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(
    help=(
        "Inspect optional LSP integration readiness. "
        "Detection-only: DevCouncil checks which language servers are on PATH; "
        "it does not run an LSP client or wire protocol."
    ),
)
logger = logging.getLogger(__name__)


@app.command("inspect")
def inspect(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
    json_format: bool = typer.Option(False, "--json", help="Emit {mode, servers_detected, note} JSON."),
):
    """Print detected language servers (detection-only; no LSP wire protocol)."""
    root = project_root.expanduser().resolve()
    # Existence check FIRST: set_log_dir/log_stage create .devcouncil/ under the
    # root, which would materialize a missing/typo'd path and turn the "does not
    # exist" report into a false success.
    if not root.exists():
        typer.echo(dump_json({"languages": [], "servers": [], "initialize_requests": {}, "error": f"{root} does not exist"}, indent=2))
        raise typer.Exit(code=1)
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev lsp inspect")
    with log_stage("lsp", project_root=root):
        log_step("lsp/1: inspecting language servers", project_root=root, trace=True)
        summary = LspInspector(root).summary()
        if json_format:
            compact = {
                "mode": summary.get("mode", "detection-only"),
                "servers_detected": summary.get("servers_detected", summary.get("detected_servers", [])),
                "note": summary.get("note", LspInspector._DETECTION_ONLY_NOTE),
            }
            typer.echo(dump_json(compact, indent=2))
        else:
            typer.echo(dump_json(summary, indent=2))
        log_step("lsp/complete", project_root=root, trace=True)
