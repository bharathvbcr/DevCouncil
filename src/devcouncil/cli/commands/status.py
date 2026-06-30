from rich.console import Console
from rich.table import Table
from rich.panel import Panel
from pathlib import Path
import json
import typer
from devcouncil.cli.commands.init import initialize_project
from devcouncil.app.project_status import compute_phase
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository
from devcouncil.telemetry.cost import group_cost
from devcouncil.live.summary import live_review_summary

console = Console()

def _status_payload(project_root: Path) -> dict:
    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"initialized": False, "phase": "UNINITIALIZED"}

    with db.get_session() as session:
        graph_repo = ArtifactGraphRepository(session)
        graph = graph_repo.load_graph()
        summary = graph.coverage_summary()

        blocking_gaps = graph.blocking_gaps()
        state = StateRepository(session).get_state()
        phase = compute_phase(graph, state.current_phase if state else None)

        # Single read of the model-call ledger: derive both the grand total and the
        # per-task breakdown from one pass (group_cost -> read_cost_records). This is
        # provider-aware (ollama records are free), matching the Cost-by-Task table.
        cost = group_cost(project_root)

        status_counts: dict[str, int] = {}
        for task in graph.tasks.values():
            status_counts[task.status] = status_counts.get(task.status, 0) + 1

        return {
            "initialized": True,
            "phase": phase,
            "coverage_summary": summary,
            "total_cost": cost["total_cost"],
            "cost_by_task": cost["by_task"],
            "task_status_counts": status_counts,
            "blocking_gaps": [gap.model_dump() for gap in blocking_gaps],
            "live_review": live_review_summary(project_root),
        }


def status(
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    fail_on_blocking: bool = typer.Option(
        False,
        "--fail-on-blocking",
        help="Exit non-zero when blocking gaps remain, so shell-driven agents can gate on $?.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Show the current status of the DevCouncil project.
    """
    root = project_root.expanduser().resolve()
    payload = _status_payload(root)

    def _maybe_fail() -> None:
        if fail_on_blocking and payload.get("blocking_gaps"):
            raise typer.Exit(code=1)

    if json_format:
        typer.echo(json.dumps(payload, indent=2))
        _maybe_fail()
        return

    if not payload["initialized"]:
        console.print("[yellow]DevCouncil state is not available in this directory.[/yellow]")
        return

    summary = payload["coverage_summary"]
    phase = payload["phase"]
    phase_colors = {
        "NEW": "yellow",
        "REQUIREMENTS_DRAFTED": "cyan",
        "PLAN_APPROVED": "green",
        "TASK_EXECUTING": "blue",
        "TASK_BLOCKED": "red",
        "PROJECT_DONE": "green bold",
    }
    phase_color = phase_colors.get(phase, "white")

    console.print(Panel(
        f"[bold]Phase:[/bold] [{phase_color}]{phase}[/{phase_color}]\n"
        f"[bold]Requirements:[/bold] {summary['total_requirements']} ({summary['requirements_without_tasks']} unmapped)\n"
        f"[bold]Tasks:[/bold] {summary['total_tasks']} ({summary['tasks_without_requirements']} orphaned)\n"
        f"[bold]Acceptance Criteria:[/bold] {summary['total_ac']} ({summary['ac_without_evidence']} unverified)\n"
        f"[bold]Gaps:[/bold] {summary['total_gaps']} ({summary['blocking_gaps']} blocking)\n"
        f"[bold]Live Review:[/bold] {payload['live_review']['cards']['critical_open']} open critical, "
        f"{len(payload['live_review']['blocking_cards'])} blocking in scope, "
        f"{payload['live_review']['pending_signals']} pending signal(s)\n"
        f"[bold]Total Cost:[/bold] ${payload['total_cost']:.4f}",
        title="DevCouncil Status",
        expand=False,
    ))

    if payload["task_status_counts"]:
        table = Table(title="Task Summary")
        table.add_column("Status", style="magenta")
        table.add_column("Count", justify="right")
        for state, count in sorted(payload["task_status_counts"].items()):
            table.add_row(state, str(count))
        console.print(table)

    cost_groups = payload.get("cost_by_task") or {}
    if cost_groups:
        cost_table = Table(title="Cost by Task")
        cost_table.add_column("Task", style="cyan")
        cost_table.add_column("Cost ($)", justify="right")
        cost_table.add_column("Calls", justify="right")
        for name, stats in sorted(cost_groups.items(), key=lambda kv: kv[1]["cost"], reverse=True):
            cost_table.add_row(name, f"{stats['cost']:.4f}", str(stats["calls"]))
        console.print(cost_table)

    blocking_gaps = payload["blocking_gaps"]
    if blocking_gaps:
        console.print(f"\n[red bold]WARNING: {len(blocking_gaps)} blocking gap(s) must be resolved:[/red bold]")
        for gap in blocking_gaps[:5]:
            console.print(f"  - [red]{gap['id']}[/red]: {gap['description'][:80]}")
        if len(blocking_gaps) > 5:
            console.print(f"  ... and {len(blocking_gaps) - 5} more. Run [bold]dev report[/bold] for details.")

    _maybe_fail()
