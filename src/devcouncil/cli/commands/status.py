from rich.console import Console
from rich.table import Table
from rich.panel import Panel
from pathlib import Path
import json
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository
from devcouncil.telemetry.cost import CostEstimator

console = Console()

def status():
    """
    Show the current status of the DevCouncil project.
    """
    db = get_db()
    if not db:
        console.print("[yellow]DevCouncil not initialized in this directory.[/yellow]")
        console.print("Run [bold]dev init[/bold] to get started.")
        return

    with db.get_session() as session:
        graph_repo = ArtifactGraphRepository(session)
        graph = graph_repo.load_graph()
        summary = graph.coverage_summary()

        reqs = list(graph.requirements.values())
        tasks = list(graph.tasks.values())
        blocking_gaps = graph.blocking_gaps()

        # Determine phase
        if not reqs and not tasks:
            phase = "NEW"
        elif reqs and not tasks:
            phase = "REQUIREMENTS_DRAFTED"
        elif blocking_gaps:
            phase = "TASK_BLOCKED"
        elif tasks:
            statuses = {t.status for t in tasks}
            if "running" in statuses:
                phase = "TASK_EXECUTING"
            elif "blocked" in statuses:
                phase = "TASK_BLOCKED"
            elif all(s in ("verified", "done") for s in statuses):
                phase = "PROJECT_DONE"
            else:
                phase = "PLAN_APPROVED"
        else:
            phase = "NEW"

        # Phase color
        phase_colors = {
            "NEW": "yellow",
            "REQUIREMENTS_DRAFTED": "cyan",
            "PLAN_APPROVED": "green",
            "TASK_EXECUTING": "blue",
            "TASK_BLOCKED": "red",
            "PROJECT_DONE": "green bold",
        }
        phase_color = phase_colors.get(phase, "white")

        # Calculate cost
        total_cost = 0.0
        log_file = Path(".devcouncil/logs/model_calls.jsonl")
        if log_file.exists():
            with open(log_file, "r", encoding="utf-8") as f:
                for line in f:
                    try:
                        entry = json.loads(line)
                        total_cost += CostEstimator.estimate_cost(
                            entry.get("response", {}).get("model", ""),
                            entry.get("usage", {})
                        )
                    except Exception:
                        continue

        console.print(Panel(
            f"[bold]Phase:[/bold] [{phase_color}]{phase}[/{phase_color}]\n"
            f"[bold]Requirements:[/bold] {summary['total_requirements']} ({summary['requirements_without_tasks']} unmapped)\n"
            f"[bold]Tasks:[/bold] {summary['total_tasks']} ({summary['tasks_without_requirements']} orphaned)\n"
            f"[bold]Acceptance Criteria:[/bold] {summary['total_ac']} ({summary['ac_without_evidence']} unverified)\n"
            f"[bold]Gaps:[/bold] {summary['total_gaps']} ({summary['blocking_gaps']} blocking)\n"
            f"[bold]Total Cost:[/bold] ${total_cost:.4f}",
            title="DevCouncil Status",
            expand=False,
        ))

        if tasks:
            table = Table(title="Task Summary")
            table.add_column("Status", style="magenta")
            table.add_column("Count", justify="right")

            status_counts: dict[str, int] = {}
            for t in tasks:
                status_counts[t.status] = status_counts.get(t.status, 0) + 1
            for s, count in sorted(status_counts.items()):
                table.add_row(s, str(count))
            console.print(table)

        if blocking_gaps:
            console.print(f"\n[red bold]WARNING: {len(blocking_gaps)} blocking gap(s) must be resolved:[/red bold]")
            for g in blocking_gaps[:5]:
                console.print(f"  - [red]{g.id}[/red]: {g.description[:80]}")
            if len(blocking_gaps) > 5:
                console.print(f"  ... and {len(blocking_gaps) - 5} more. Run [bold]dev report[/bold] for details.")
