import logging
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table
from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

console = Console()
logger = logging.getLogger(__name__)


def _gaps_payload(
    project_root: Path,
    *,
    blocking_only: bool = False,
    task_id: str | None = None,
) -> dict:
    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"initialized": False, "gaps": [], "blocking_count": 0, "advisory_count": 0}

    with db.get_session() as session:
        if task_id:
            from devcouncil.storage.repositories import GapRepository

            gaps = GapRepository(session).get_for_task(task_id)
            if blocking_only:
                gaps = [g for g in gaps if g.blocking]
            return {
                "initialized": True,
                "ok": True,
                "task_id": task_id,
                "gaps": [gap.model_dump() for gap in gaps],
                "blocking_count": sum(1 for g in gaps if g.blocking),
            }

        graph = ArtifactGraphRepository(session).load_graph()
        if blocking_only:
            gaps = graph.blocking_gaps()
        else:
            gaps = sorted(graph.gaps.values(), key=lambda g: (not g.blocking, g.id))
        blocking = [g for g in gaps if g.blocking]
        advisory = [g for g in gaps if not g.blocking]
        return {
            "initialized": True,
            "gaps": [gap.model_dump() for gap in gaps],
            "blocking_count": len(blocking),
            "advisory_count": len(advisory),
            "total_count": len(gaps),
        }


def _next_actions_payload(project_root: Path, task_id: str) -> dict:
    from devcouncil.integrations.mcp.util import allowed_next_tools
    from devcouncil.storage.repositories import GapRepository, TaskRepository
    from devcouncil.verification.next_actions import split_next_actions

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory."}

    with db.get_session() as session:
        gaps = GapRepository(session).get_for_task(task_id)
        task = TaskRepository(session).get_by_id(task_id)
    blocking_actions, advisory_actions = split_next_actions(gaps)
    has_blocking = any(g.blocking for g in gaps)
    return {
        "ok": True,
        "task_id": task_id,
        "next_actions": [a.model_dump() for a in blocking_actions],
        "advisory_actions": [a.model_dump() for a in advisory_actions],
        "allowed_next_tools": allowed_next_tools(task.status if task else "planned", has_blocking),
    }


def gaps(
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    blocking_only: bool = typer.Option(
        False,
        "--blocking-only",
        help="Show only blocking gaps.",
    ),
    task_id: str | None = typer.Option(
        None,
        "--task-id",
        help="Scope gaps to a single task (MCP-compatible payload).",
    ),
    next_actions: bool = typer.Option(
        False,
        "--next-actions",
        help="Emit repair next-actions for --task-id instead of raw gaps.",
    ),
    fail_on_blocking: bool = typer.Option(
        False,
        "--fail-on-blocking",
        help="Exit non-zero when blocking gaps remain.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """List all verification gaps (blocking and advisory)."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info(
        "dev gaps: json=%s blocking_only=%s fail_on_blocking=%s",
        json_format,
        blocking_only,
        fail_on_blocking,
    )

    with log_stage("gaps", project_root=root):
        log_step("gaps/1: loading gaps", project_root=root, trace=True)
        if next_actions:
            if not task_id:
                message = "--next-actions requires --task-id"
                if json_format:
                    typer.echo(dump_json({"ok": False, "error": message}, indent=2))
                else:
                    console.print(f"[red]{message}[/red]")
                raise typer.Exit(code=1)
            payload = _next_actions_payload(root, task_id)
        else:
            payload = _gaps_payload(root, blocking_only=blocking_only, task_id=task_id)

        def _maybe_fail() -> None:
            if fail_on_blocking and payload.get("blocking_count", 0) > 0:
                raise typer.Exit(code=1)

        if json_format:
            typer.echo(dump_json(payload, indent=2))
            _maybe_fail()
            log_step("gaps/complete", project_root=root, trace=True)
            return

        if not payload["initialized"]:
            console.print("[yellow]DevCouncil state is not available in this directory.[/yellow]")
            log_step("gaps/complete", project_root=root, trace=True)
            return

        table = Table(title="DevCouncil Gaps")
        table.add_column("ID", style="cyan", no_wrap=True)
        table.add_column("Blocking", justify="center")
        table.add_column("Severity")
        table.add_column("Type")
        table.add_column("Task")
        table.add_column("Description")

        for gap in payload["gaps"]:
            blocking_style = "[red]yes[/red]" if gap.get("blocking") else "[dim]no[/dim]"
            desc = (gap.get("description") or "")[:80]
            table.add_row(
                gap.get("id", ""),
                blocking_style,
                gap.get("severity", ""),
                gap.get("gap_type", ""),
                gap.get("task_id") or "",
                desc,
            )

        console.print(table)
        console.print(
            f"\n[bold]{payload['total_count']}[/bold] gap(s): "
            f"[red]{payload['blocking_count']} blocking[/red], "
            f"[dim]{payload['advisory_count']} advisory[/dim]"
        )

        _maybe_fail()
        log_step("gaps/complete", project_root=root, trace=True)
