"""`dev provenance` and `dev resource` — audit trail and MCP corpus reads."""

from __future__ import annotations

import logging
from pathlib import Path

import typer

from devcouncil.reporting.mcp_resources import list_mcp_resource_uris, read_mcp_resource
from devcouncil.reporting.task_provenance import task_provenance_payload
from devcouncil.telemetry.logging_setup import set_log_dir
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

resource_app = typer.Typer(help="Read MCP corpus resources through the CLI service layer.")
logger = logging.getLogger(__name__)


def provenance(
    task_id: str = typer.Argument(..., help="Task ID to inspect."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Show file changes, verification runs, and diff-coverage evidence for a task."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    logger.info("dev provenance: task=%s json=%s", task_id, json_format)

    with log_stage("provenance", project_root=root, task_id=task_id):
        payload = task_provenance_payload(root, task_id)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("provenance/complete", project_root=root, task_id=task_id, trace=True)
            if not payload.get("ok", True):
                raise typer.Exit(code=1)
            return

        if not payload.get("ok", True):
            typer.echo(payload.get("error", "provenance unavailable"))
            raise typer.Exit(code=1)

        typer.echo(
            f"Task {task_id}: {len(payload.get('file_changes', []))} file change(s), "
            f"{len(payload.get('verification_runs', []))} verification run(s), "
            f"{len(payload.get('diff_coverage', []))} diff-coverage record(s)."
        )
        log_step("provenance/complete", project_root=root, task_id=task_id, trace=True)


@resource_app.command("list")
def resource_list(
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """List browsable MCP corpus resource URIs."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    resources = list_mcp_resource_uris(root)
    if json_format:
        typer.echo(dump_json({"resources": resources}, indent=2))
        return
    for item in resources:
        typer.echo(f"{item['uri']}  ({item['mimeType']})  {item['name']}")


@resource_app.command("read")
def resource_read(
    uri: str = typer.Argument(..., help="Resource URI, e.g. devcouncil://tasks"),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Read one MCP corpus resource (stdout body)."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    try:
        typer.echo(read_mcp_resource(root, uri), nl=False)
    except ValueError as exc:
        typer.echo(str(exc))
        raise typer.Exit(code=1) from exc
