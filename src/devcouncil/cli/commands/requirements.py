from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.task import Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import RequirementRepository, TaskRepository
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def _derive_requirement_status(req_id: str, tasks: list[Task]) -> str:
    linked = [task for task in tasks if req_id in task.requirement_ids]
    if not linked:
        return "unmapped"
    statuses = {task.status for task in linked}
    if "blocked" in statuses:
        return "blocked"
    if statuses <= {"verified", "done"}:
        return "verified"
    if "running" in statuses:
        return "in_progress"
    return "planned"


def _requirements_payload(project_root: Path) -> dict:
    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"initialized": False, "requirements": []}

    with db.get_session() as session:
        requirements = RequirementRepository(session).get_all()
        tasks = TaskRepository(session).get_all()
        rows = []
        for req in requirements:
            linked_count = sum(1 for task in tasks if req.id in task.requirement_ids)
            rows.append(
                {
                    "id": req.id,
                    "title": req.title,
                    "priority": req.priority,
                    "status": _derive_requirement_status(req.id, tasks),
                    "linked_task_count": linked_count,
                    "requirement": req.model_dump(),
                }
            )
        rows.sort(key=lambda row: (row["priority"], row["id"]))
        return {
            "initialized": True,
            "requirements": rows,
            "total_count": len(rows),
        }


def requirements(
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """List requirements with priority, status, and linked task counts."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    logger.info("dev requirements: json=%s", json_format)

    with log_stage("requirements", project_root=root):
        log_step("requirements/1: loading requirements", project_root=root, trace=True)
        payload = _requirements_payload(root)

        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("requirements/complete", project_root=root, trace=True)
            return

        if not payload["initialized"]:
            console.print("[yellow]DevCouncil state is not available in this directory.[/yellow]")
            log_step("requirements/complete", project_root=root, trace=True)
            return

        table = Table(title="DevCouncil Requirements")
        table.add_column("ID", style="cyan", no_wrap=True)
        table.add_column("Priority")
        table.add_column("Status", style="magenta")
        table.add_column("Linked Tasks", justify="right")

        for row in payload["requirements"]:
            table.add_row(
                row["id"],
                row["priority"],
                row["status"],
                str(row["linked_task_count"]),
            )

        console.print(table)
        console.print(f"\n[bold]{payload['total_count']}[/bold] requirement(s)")
        log_step("requirements/complete", project_root=root, trace=True)
