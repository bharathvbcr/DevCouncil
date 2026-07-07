"""Export DevCouncil project state to a portable JSON snapshot (no secrets)."""

from __future__ import annotations

from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path

from devcouncil.utils.json_persist import write_json

import typer
from rich.console import Console

from devcouncil.cli.commands.gaps import _gaps_payload
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import RequirementRepository, TaskRepository
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)

EXPORT_RELATIVE = Path(".devcouncil") / "export" / "state.json"


def _export_payload(project_root: Path) -> dict:
    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"initialized": False, "requirements": [], "tasks": [], "gaps": {}}

    with db.get_session() as session:
        requirements = [req.model_dump() for req in RequirementRepository(session).get_all()]
        tasks = [task.model_dump() for task in TaskRepository(session).get_all()]
    gaps = _gaps_payload(project_root)
    return {
        "initialized": True,
        "requirements": requirements,
        "tasks": tasks,
        "gaps": {
            "blocking_count": gaps.get("blocking_count", 0),
            "advisory_count": gaps.get("advisory_count", 0),
            "total_count": gaps.get("total_count", 0),
            "items": gaps.get("gaps", []),
        },
    }


def export_state(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    output: Path | None = typer.Option(None, "--output", "-o", help="Output path (default: .devcouncil/export/state.json)."),
    json_format: bool = typer.Option(False, "--json", help="Print payload to stdout instead of writing a file."),
):
    """Write requirements, tasks, and gaps summary to .devcouncil/export/state.json."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    logger.info("dev export: json=%s", json_format)

    with log_stage("export", project_root=root):
        log_step("export/1: building snapshot", project_root=root, trace=True)
        payload = _export_payload(root)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("export/complete", project_root=root, trace=True)
            return

        dest = (output or (root / EXPORT_RELATIVE)).expanduser().resolve()
        dest.parent.mkdir(parents=True, exist_ok=True)
        write_json(dest, payload)
        console.print(f"[green]Exported DevCouncil state[/green] to {dest}")
        log_step("export/complete", project_root=root, path=str(dest), trace=True)
