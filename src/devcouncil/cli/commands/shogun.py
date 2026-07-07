"""``dev shogun`` — run the planned task graph as a feudal, parallel campaign.

The Shogun relays your goal to the Karo, who dispatches the plan's tasks to a
pool of Ashigaru in parallel (dependency-aware) and routes each finished task to
the Gunshi for verification. It reuses DevCouncil's real executors and Verifier;
without a ``--executor`` it performs a dry run so you can preview the battle plan.

    dev shogun run "Ship the settings page" --executor claude --ashigaru 4
    dev shogun status          # print the campaign dashboard
    dev shogun inbox karo      # read an agent's mailbox
    dev shogun roster          # show the chain of command
"""

from __future__ import annotations

from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path
from typing import List, Optional

import typer
from rich.console import Console
from rich.panel import Panel
from rich.table import Table

from devcouncil.shogun import (
    Mailbox,
    ROLES,
    Rank,
    ShogunCampaign,
    build_coding_executor_factory,
    build_verifier_fn,
)
from devcouncil.shogun.notify import Notifier, NullNotifier
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Command an AI army: run the plan as a Shogun→Karo→Ashigaru→Gunshi campaign.")
console = Console()
logger = logging.getLogger(__name__)

# Task statuses that still need work (everything else is a satisfied prerequisite).
_ACTIONABLE = {"planned", "ready", "blocked", "running"}


def _load_plan(root: Path):
    """Return ``(all_tasks, requirements)`` from the DevCouncil store."""
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    db = get_db(root)
    if not db:
        return [], []
    with db.get_session() as session:
        tasks = TaskRepository(session).get_all()
        reqs = RequirementRepository(session).get_all()
    return tasks, reqs


def _persist_statuses(root: Path, tasks) -> None:
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    db = get_db(root)
    if not db:
        return
    with db.get_session() as session:
        repo = TaskRepository(session)
        for task in tasks:
            repo.save(task)


@app.command("run")
def run_campaign(
    goal: str = typer.Argument(..., help="The order to give the Shogun (a natural-language goal)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    ashigaru: int = typer.Option(4, "--ashigaru", "-n", min=1, help="Number of Ashigaru (worker slots)."),
    max_parallel: int = typer.Option(4, "--max-parallel", "-p", min=1, help="Max tasks dispatched at once."),
    executor: Optional[str] = typer.Option(
        None,
        "--executor",
        "-e",
        help="Coding CLI the Ashigaru use (claude, codex, …). Omit for a dry run (no changes).",
    ),
    profile: Optional[str] = typer.Option(None, "--profile", help="Executor profile (default/yolo/prod)."),
    verify: bool = typer.Option(True, "--verify/--no-verify", help="Gunshi QC via the DevCouncil Verifier."),
    serial_verify: bool = typer.Option(
        True,
        "--serial-verify/--parallel-verify",
        help="Run Gunshi QC one task at a time (safe against git races) vs. concurrently.",
    ),
    stream: bool = typer.Option(False, "--stream", help="Stream executor output."),
    ntfy_topic: Optional[str] = typer.Option(None, "--ntfy-topic", help="Push campaign updates to this ntfy topic."),
    json_format: bool = typer.Option(False, "--json", help="Emit a machine-readable result."),
    fail_on_blocking: bool = typer.Option(
        False, "--fail-on-blocking", help="Exit non-zero if any task ends blocked (for CI/agents)."
    ),
):
    """Muster the army and execute the plan for GOAL."""
    from devcouncil.cli.commands.init import initialize_project

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    logger.info("dev shogun run: executor=%s ashigaru=%s", executor, ashigaru)

    with log_stage("shogun", project_root=root, subcommand="run"):
        initialize_project(root, quiet=True)
        log_step("shogun/1: loading plan", project_root=root, trace=True)
        tasks, reqs = _load_plan(root)
        if not tasks:
            console.print(
                "[yellow]No plan found. Run [bold]dev plan[/bold] first to muster tasks for the Shogun.[/yellow]"
            )
            raise typer.Exit(code=0)

        dry_run = executor is None
        if dry_run:
            console.print(
                "[cyan]Dry run — no --executor given. The Karo will assign and route, "
                "but Ashigaru will not touch the repo.[/cyan]"
            )
            executor_factory = None  # campaign default = echo executor
            verify_fn = None  # campaign default = pass-through QC
        else:
            executor_factory = build_coding_executor_factory(root, executor, profile=profile, stream=stream)
            if verify:
                from devcouncil.cli.commands.run import _build_verification_router

                verify_fn = build_verifier_fn(root, _build_verification_router(root))
            else:
                verify_fn = None

        notifier: Notifier = Notifier(topic=ntfy_topic) if ntfy_topic else NullNotifier()

        campaign = ShogunCampaign(
            root,
            goal=goal,
            tasks=tasks,
            requirements=reqs,
            num_ashigaru=ashigaru,
            max_parallel=max_parallel,
            executor_factory=executor_factory,
            verify_fn=verify_fn,
            notifier=notifier,
            verify_serialized=serial_verify,
            on_event=None if json_format else (lambda m: console.print(f"  {m}")),
        )

        log_step("shogun/2: dispatching", project_root=root, trace=True)
        result = campaign.run()

        if not dry_run:
            touched = {o.task_id for o in result.outcomes}
            _persist_statuses(root, [t for t in tasks if t.id in touched])

        if json_format:
            payload = {
                "goal": result.goal,
                "success": result.success,
                "verified": result.verified,
                "blocked": result.blocked,
                "skipped": result.skipped,
                "dashboard": str(result.dashboard_path) if result.dashboard_path else None,
                "outcomes": [
                    {
                        "task_id": o.task_id,
                        "title": o.title,
                        "owner": o.owner,
                        "bloom": o.bloom,
                        "status": o.status,
                        "blocking_gaps": o.blocking_gaps,
                    }
                    for o in result.outcomes
                ],
            }
            typer.echo(dump_json(payload, indent=2))
        else:
            _render_result(result)

        log_step("shogun/complete", project_root=root, trace=True)
        if fail_on_blocking and result.blocked:
            raise typer.Exit(code=1)


def _render_result(result) -> None:
    table = Table(title="⚔️  Shogun Campaign")
    table.add_column("Task", style="cyan")
    table.add_column("Owner")
    table.add_column("Bloom")
    table.add_column("Verdict")
    verdict_style = {
        "verified": "[green]verified[/green]",
        "blocked": "[red]blocked[/red]",
        "failed": "[red]failed[/red]",
        "skipped": "[yellow]skipped[/yellow]",
    }
    for o in result.outcomes:
        table.add_row(o.title[:48], o.owner, o.bloom, verdict_style.get(o.status, o.status))
    console.print(table)
    console.print(
        Panel(
            result.summary_line(),
            title="戦果 · Result",
            expand=False,
            border_style="green" if result.success else "yellow",
        )
    )
    if result.dashboard_path:
        console.print(f"[dim]Dashboard: {result.dashboard_path}[/dim]")


@app.command("status")
def status(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Print the campaign dashboard (written by the Karo)."""
    root = project_root.expanduser().resolve()
    path = root / ".devcouncil" / "shogun" / "dashboard.md"
    if not path.exists():
        console.print("[yellow]No campaign has been run yet — no dashboard to show.[/yellow]")
        raise typer.Exit(code=0)
    console.print(path.read_text(encoding="utf-8"))


@app.command("inbox")
def inbox(
    agent: str = typer.Argument(..., help="Agent id: shogun, karo, ashigaru1…, gunshi."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    unread_only: bool = typer.Option(False, "--unread", help="Show only unread messages."),
):
    """Read an agent's mailbox — the on-disk message bus."""
    root = project_root.expanduser().resolve()
    mailbox = Mailbox(root)
    messages = mailbox.unread(agent) if unread_only else mailbox.all(agent)
    if not messages:
        console.print(f"[dim]{agent}'s mailbox is empty.[/dim]")
        raise typer.Exit(code=0)
    table = Table(title=f"📨 {agent} — {len(messages)} message(s)")
    table.add_column("From", style="cyan")
    table.add_column("Type")
    table.add_column("Content")
    table.add_column("Read", justify="center")
    for m in messages:
        table.add_row(m.from_agent, m.type, m.content[:60], "•" if m.read else "[yellow]![/yellow]")
    console.print(table)


@app.command("roster")
def roster():
    """Show the chain of command and each rank's remit."""
    order: List[Rank] = [Rank.SHOGUN, Rank.KARO, Rank.ASHIGARU, Rank.GUNSHI]
    table = Table(title="Shogun Chain of Command")
    table.add_column("Rank", style="magenta")
    table.add_column("Title")
    table.add_column("Reports to")
    table.add_column("Remit")
    for rank in order:
        role = ROLES[rank]
        table.add_row(
            rank.value,
            role.title_en,
            role.reports_to.value if role.reports_to else "the Lord",
            role.summary,
        )
    console.print(table)
