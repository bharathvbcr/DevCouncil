import typer
import asyncio
from rich.console import Console
from rich.table import Table
from pathlib import Path
from typing import Optional
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository, GapRepository, EvidenceRepository, StateRepository
from devcouncil.verification.verifier import Verifier
from devcouncil.llm.provider import OpenRouterProvider
from devcouncil.llm.router import ModelRouter
from devcouncil.domain.evidence import CommandResult, DiffEvidence, TestEvidence
from devcouncil.app.config import load_config, get_api_key
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.telemetry.traces import TraceLogger

console = Console()
MAX_RENDERED_GAPS = 20

def verify(
    task_id: Optional[str] = typer.Argument(None, help="Optional ID of the task to verify"),
):
    """
    Verify one task, or all tasks when TASK_ID is omitted.
    """
    db = get_db()
    if not db:
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        return

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        req_repo = RequirementRepository(session)
        gap_repo = GapRepository(session)
        evidence_repo = EvidenceRepository(session)
        
        tasks = [task_repo.get_by_id(task_id)] if task_id else task_repo.get_all()
        tasks = [task for task in tasks if task is not None]
        if not tasks:
            missing = f"Task {task_id} not found." if task_id else "No tasks found to verify."
            console.print(f"[red]{missing}[/red]")
            return

        reqs = req_repo.get_all()
        
        # Load router for LLM review if possible
        router = None
        try:
            config = load_config(Path("."))
            api_key = get_api_key(config.models.provider)
            provider = OpenRouterProvider(api_key)
            role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
            router = ModelRouter(provider, role_config)
        except Exception:
            pass

        verifier = Verifier(Path("."), router=router)
        total_gaps = 0
        blocked_tasks = 0

        for task in tasks:
            TraceLogger(Path(".")).log_event(
                "task_verification_started",
                {"task_id": task.id},
                task_id=task.id,
                summary=f"Verifying {task.id}",
            )
            graph_context = CodeReviewGraphAdapter(Path(".")).get_context(
                [planned.path for planned in task.planned_files]
            )
            if graph_context.available:
                TraceLogger(Path(".")).log_event(
                    "graph_context_loaded",
                    graph_context.model_dump(),
                    task_id=task.id,
                    summary=f"Loaded graph context for {task.id}",
                )
            StateRepository(session).record_phase(ProjectPhase.TASK_VERIFYING.value)
            gap_repo.delete_for_task(task.id)
            evidence_repo.delete_for_task(task.id)

            gaps, evidence = asyncio.run(verifier.verify_task(task, reqs))
            total_gaps += len(gaps)

            for gap in gaps:
                gap_repo.save(gap)

            for ev in evidence:
                if isinstance(ev, CommandResult):
                    evidence_repo.save_command_result(task.id, ev)
                elif isinstance(ev, DiffEvidence):
                    evidence_repo.save_diff_evidence(ev)
                elif isinstance(ev, TestEvidence):
                    evidence_repo.save_test_evidence(ev, task.id)

            _print_task_result(task.id, gaps)

            if any(gap.blocking for gap in gaps):
                task.status = "blocked"
                blocked_tasks += 1
                TraceLogger(Path(".")).log_event(
                    "gate_failed",
                    {"task_id": task.id, "gap_count": len(gaps)},
                    task_id=task.id,
                    summary=f"{task.id} blocked with {len(gaps)} gap(s)",
                )
            else:
                task.status = "verified"
                TraceLogger(Path(".")).log_event(
                    "task_verified",
                    {"task_id": task.id, "gap_count": len(gaps)},
                    task_id=task.id,
                    summary=f"{task.id} verified",
                )
            task_repo.save(task)

        StateRepository(session).record_phase(
            ProjectPhase.TASK_BLOCKED.value if blocked_tasks else ProjectPhase.TASK_VERIFIED.value
        )

        if len(tasks) > 1:
            if blocked_tasks:
                console.print(
                    f"\n[yellow]Verified {len(tasks)} tasks: {blocked_tasks} blocked, "
                    f"{total_gaps} total gap(s).[/yellow]"
                )
            else:
                console.print(f"\n[green]Verified {len(tasks)} tasks successfully.[/green]")


def _print_task_result(task_id: str, gaps):
    if not gaps:
        console.print(f"[green]Task {task_id} verified successfully! No gaps found.[/green]")
        return

    console.print(f"[yellow]Verification finished for task {task_id} with {len(gaps)} gaps:[/yellow]")

    table = Table(title="Detected Gaps")
    table.add_column("ID", style="cyan")
    table.add_column("Severity", style="magenta")
    table.add_column("Description", style="white")
    table.add_column("Blocking", style="red")

    for gap in gaps[:MAX_RENDERED_GAPS]:
        table.add_row(
            gap.id,
            gap.severity,
            gap.description,
            "YES" if gap.blocking else "NO",
        )

    console.print(table)
    if len(gaps) > MAX_RENDERED_GAPS:
        console.print(
            f"[yellow]Showing first {MAX_RENDERED_GAPS} of {len(gaps)} gaps. "
            "Run [bold]dev report --json[/bold] for the full list.[/yellow]"
        )

    if any(gap.blocking for gap in gaps):
        console.print(f"\n[red]Task {task_id} is BLOCKED due to critical gaps.[/red]")
    else:
        console.print(f"\n[green]Task {task_id} passed with non-blocking gaps.[/green]")
