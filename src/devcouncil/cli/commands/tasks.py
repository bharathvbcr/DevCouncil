from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path
from typing import Literal, cast

import typer
from rich.console import Console
from rich.table import Table
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.telemetry.traces import TraceLogger

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)


def _lease_public_view(lease, *, expired: bool) -> dict:
    return {
        "owner": lease.owner,
        "agent": lease.agent,
        "expires_at": lease.expires_at,
        "expired": expired,
    }


def _active_leases_by_task(session) -> dict[str, tuple[object, bool]]:
    return {
        lease.task_id: (lease, expired)
        for lease, expired in TaskLeaseRepository(session).list_leases(active_only=True)
    }


def _task_row_payload(task, leases_by_task: dict[str, tuple[object, bool]]) -> dict:
    payload = task.model_dump()
    lease_pair = leases_by_task.get(task.id)
    payload["lease"] = _lease_public_view(lease_pair[0], expired=lease_pair[1]) if lease_pair else None
    return payload


@app.callback(invoke_without_command=True)
def tasks(
    ctx: typer.Context,
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    status: str | None = typer.Option(None, "--status", help="Filter tasks by status."),
    limit: int = typer.Option(100, "--limit", min=1, max=500, help="Max tasks to return (JSON pagination)."),
    offset: int = typer.Option(0, "--offset", min=0, help="Skip this many tasks before returning results."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    List task graph and task gate status.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev tasks: json=%s", json_format)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    with log_stage("tasks", project_root=root):
        log_step("tasks/1: loading task graph", project_root=root, trace=True)
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            tasks_list = task_repo.get_all()
            if status:
                tasks_list = [t for t in tasks_list if t.status == status]
            total = len(tasks_list)
            window = tasks_list[offset:offset + limit]
            leases_by_task = _active_leases_by_task(session)

            if json_format:
                typer.echo(dump_json({
                    "tasks": [_task_row_payload(task, leases_by_task) for task in window],
                    "total": total,
                    "offset": offset,
                    "limit": limit,
                    "returned": len(window),
                }, indent=2))
                log_step("tasks/complete", project_root=root, count=len(window), trace=True)
                return

            tasks_list = window

            if not tasks_list:
                console.print("No tasks found. Run 'dev plan' to generate tasks.")
                log_step("tasks/complete", project_root=root, count=0, trace=True)
                return

            table = Table(title="DevCouncil Tasks")
            table.add_column("Task ID", style="cyan", no_wrap=True)
            table.add_column("Title", style="white")
            table.add_column("Status", style="magenta")
            table.add_column("Priority", style="yellow")
            table.add_column("Lease", style="blue")
            table.add_column("Linked Reqs", style="green")

            for t in tasks_list:
                reqs = ", ".join(t.requirement_ids)
                priority = t.priority or "-"
                lease_pair = leases_by_task.get(t.id)
                lease_label = lease_pair[0].owner if lease_pair else "—"
                table.add_row(t.id, t.title, t.status, priority, lease_label, reqs)

            console.print(table)
        log_step("tasks/complete", project_root=root, count=len(tasks_list), trace=True)


@app.command("cancel")
def cancel_task(
    task_id: str = typer.Argument(..., help="Task ID to cancel, e.g. TASK-001."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Cancel a task that is not already done or cancelled."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        message = "DevCouncil state is unavailable in this directory."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)

    with log_stage("tasks", project_root=root, subcommand="cancel"):
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if task is None:
                message = f"Task {task_id} not found."
                if json_format:
                    typer.echo(dump_json({"ok": False, "error": message}, indent=2))
                else:
                    console.print(f"[red]{message}[/red]")
                raise typer.Exit(code=1)
            if task.status in {"done", "cancelled"}:
                message = f"Task {task_id} cannot be cancelled (status={task.status})."
                if json_format:
                    typer.echo(dump_json({"ok": False, "error": message, "task_id": task_id, "status": task.status}, indent=2))
                else:
                    console.print(f"[red]{message}[/red]")
                raise typer.Exit(code=2)
            previous_status = task.status
            task.status = "cancelled"
            task_repo.save(task)
        TraceLogger(root).log_event(
            "task_cancelled",
            {"task_id": task_id, "previous_status": previous_status},
            task_id=task_id,
            summary=f"Cancelled {task_id} (was {previous_status})",
        )
        payload = {"ok": True, "task_id": task_id, "status": "cancelled", "previous_status": previous_status}
        if json_format:
            typer.echo(dump_json(payload, indent=2))
        else:
            console.print(f"[green]Cancelled {task_id}[/green] (was {previous_status})")


VALID_PRIORITIES = frozenset({"high", "medium", "low"})


@app.command("reprioritize")
def reprioritize_task(
    task_id: str = typer.Argument(..., help="Task ID to reprioritize, e.g. TASK-001."),
    priority: str = typer.Option(..., "--priority", help="New priority: high, medium, or low."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Set or update a task's optional priority hint."""
    normalized = priority.strip().lower()
    if normalized not in VALID_PRIORITIES:
        message = f"--priority must be one of: {', '.join(sorted(VALID_PRIORITIES))}."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=2)

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        message = "DevCouncil state is unavailable in this directory."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)

    with log_stage("tasks", project_root=root, subcommand="reprioritize"):
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if task is None:
                message = f"Task {task_id} not found."
                if json_format:
                    typer.echo(dump_json({"ok": False, "error": message}, indent=2))
                else:
                    console.print(f"[red]{message}[/red]")
                raise typer.Exit(code=1)
            previous = task.priority
            task.priority = cast(Literal["high", "medium", "low"], normalized)
            task_repo.save(task)
        payload = {
            "ok": True,
            "task_id": task_id,
            "priority": normalized,
            "previous_priority": previous,
        }
        if json_format:
            typer.echo(dump_json(payload, indent=2))
        else:
            prev_label = previous or "(none)"
            console.print(f"[green]Set {task_id} priority[/green] to {normalized} (was {prev_label})")


@app.command("edit")
def edit_task(
    task_id: str = typer.Argument(..., help="Task ID to edit, e.g. TASK-001."),
    title: str | None = typer.Option(None, "--title", help="New task title."),
    description: str | None = typer.Option(None, "--description", help="New task description."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Edit a task's title and/or description."""
    if title is None and description is None:
        message = "Provide at least one of --title or --description."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=2)

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        message = "DevCouncil state is unavailable in this directory."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)

    with log_stage("tasks", project_root=root, subcommand="edit"):
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if task is None:
                message = f"Task {task_id} not found."
                if json_format:
                    typer.echo(dump_json({"ok": False, "error": message}, indent=2))
                else:
                    console.print(f"[red]{message}[/red]")
                raise typer.Exit(code=1)
            changes: dict[str, tuple[str, str]] = {}
            if title is not None:
                changes["title"] = (task.title, title)
                task.title = title
            if description is not None:
                changes["description"] = (task.description, description)
                task.description = description
            task_repo.save(task)
        payload = {"ok": True, "task_id": task_id, "changes": {k: {"from": v[0], "to": v[1]} for k, v in changes.items()}}
        if json_format:
            typer.echo(dump_json(payload, indent=2))
        else:
            console.print(f"[green]Updated {task_id}[/green]: {', '.join(changes)}")
