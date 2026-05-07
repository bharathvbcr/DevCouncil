import asyncio
from pathlib import Path
from types import SimpleNamespace

import typer
from rich.console import Console

from devcouncil.cli.commands import plan as plan_command
from devcouncil.cli.commands import report as report_command
from devcouncil.cli.commands import run as run_command
from devcouncil.app.config import load_config
from devcouncil.cli.commands.init import initialize_project
from devcouncil.executors.agent_registry import AGENT_ALIASES, load_cli_agent_specs
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository, TaskRepository
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.live.summary import live_review_summary
from devcouncil.reporting.report_builder import ReportBuilder


console = Console()

SUPPORTED_EXECUTORS = {
    "codex",
    "codex-cli",
    "gemini",
    "gemini-cli",
    "claude",
    "claude-code",
    "claude-cli",
    "native",
    "native-preview",
    "mini",
    "openhands",
    "warp",
    "warp-cli",
    "oz",
    "oz-cli",
}
SUPPORTED_EXECUTORS.update(AGENT_ALIASES)

AGENT_REPORT_FILE = Path(".devcouncil/reports/latest.json")


def _normalize_executor(executor: str) -> str:
    return executor.strip().lower().replace("_", "-")


def _configured_executor(root: Path) -> str:
    try:
        configured = load_config(root).execution.default_executor
    except FileNotFoundError:
        configured = "codex"
    return _normalize_executor(configured or "codex")


def _custom_cli_agents(root: Path) -> set[str]:
    specs = load_cli_agent_specs(root)
    return {name for name, spec in specs.items() if not spec.built_in}


def _load_tasks(root: Path):
    db = get_db(root)
    if not db:
        return []
    with db.get_session() as session:
        return TaskRepository(session).get_all()


def _load_tasks_by_id(root: Path, task_ids: list[str]):
    db = get_db(root)
    if not db:
        return [], task_ids
    with db.get_session() as session:
        repo = TaskRepository(session)
        tasks = []
        missing = []
        for task_id in task_ids:
            task = repo.get_by_id(task_id)
            if task is None:
                missing.append(task_id)
            else:
                tasks.append(task)
        return tasks, missing


def _unique_task_ids(task_ids: list[str]) -> list[str]:
    seen = set()
    unique = []
    for task_id in task_ids:
        if task_id in seen:
            continue
        seen.add(task_id)
        unique.append(task_id)
    return unique


def _record_project_done(root: Path) -> None:
    db = get_db(root)
    if not db:
        return
    with db.get_session() as session:
        StateRepository(session).record_phase(ProjectPhase.PROJECT_DONE.value)


def _record_project_blocked(root: Path) -> None:
    db = get_db(root)
    if not db:
        return
    with db.get_session() as session:
        StateRepository(session).record_phase(ProjectPhase.TASK_BLOCKED.value)


def _render_final_report(root: Path, json_report: bool) -> str:
    db = get_db(root)
    if not db:
        raise RuntimeError("DevCouncil state is unavailable in this directory.")
    with db.get_session() as session:
        graph = ArtifactGraphRepository(session).load_graph()
    live_review = live_review_summary(root)
    if json_report:
        return ReportBuilder.build_json(graph, live_review=live_review)
    return ReportBuilder.build_markdown(graph, live_review=live_review)


def _write_report_file(root: Path, report_file: Path, content: str) -> Path:
    path = report_file.expanduser()
    if not path.is_absolute():
        path = root / path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return path


def _command_label(ctx: typer.Context) -> str:
    command = ctx.info_name or "e2e"
    return f"dev {command}"


def go(
    ctx: typer.Context,
    goal: str = typer.Argument(..., help="Implementation goal to plan, execute, verify, and report."),
    executor: str | None = typer.Option(
        None,
        "--executor",
        "-e",
        help="Automated executor to use. Defaults to execution.default_executor in .devcouncil/config.yaml.",
    ),
    dry_run: bool = typer.Option(False, "--dry-run", help="Use mock planning responses for local smoke testing."),
    continue_on_blocked: bool = typer.Option(
        False,
        "--continue-on-blocked",
        help="Continue later tasks even if an earlier task is blocked by verification.",
    ),
    json_report: bool = typer.Option(False, "--json-report", "--json", help="Print the final report as JSON."),
    report_file: Path | None = typer.Option(
        None,
        "--report-file",
        help="Write the final report to a file. Relative paths resolve from --project-root.",
    ),
    agent: bool = typer.Option(
        False,
        "--agent",
        help="Use coding-agent defaults: JSON report plus .devcouncil/reports/latest.json.",
    ),
    profile: str | None = typer.Option(None, "--profile", help="CLI-agent execution profile to pass to dev run."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Run the full DevCouncil loop in one command.
    """
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    if agent:
        json_report = True
        if report_file is None:
            report_file = AGENT_REPORT_FILE

    normalized_executor = _normalize_executor(executor) if executor else _configured_executor(root)
    command_label = _command_label(ctx)
    if normalized_executor == "manual":
        console.print(
            f"[red]`{command_label}` requires an automated executor. "
            "Use `dev run TASK-ID --executor manual` for handoff mode.[/red]"
        )
        raise typer.Exit(code=2)
    supported = SUPPORTED_EXECUTORS | _custom_cli_agents(root)
    if normalized_executor not in supported:
        console.print(
            f"[red]Unsupported executor for `{command_label}`: "
            f"{normalized_executor}. Supported: {', '.join(sorted(supported))}.[/red]"
        )
        raise typer.Exit(code=2)

    console.print(f"[bold]Planning goal:[/bold] {goal}")
    planned_task_ids = asyncio.run(plan_command.run_plan_flow(goal, dry_run=dry_run, persist=True, project_root=root))

    task_ids = _unique_task_ids(planned_task_ids or [])
    tasks, missing_task_ids = _load_tasks_by_id(root, task_ids)
    if missing_task_ids:
        console.print(f"[red]Planning returned task IDs that were not persisted: {', '.join(missing_task_ids)}[/red]")
        raise typer.Exit(code=1)
    if not tasks:
        console.print("[red]Planning did not produce any approved tasks.[/red]")
        raise typer.Exit(code=1)

    failed: list[str] = []
    executed_task_ids: list[str] = []
    for task in tasks:
        if task.status in {"verified", "done"}:
            console.print(f"[green]Skipping {task.id}; already {task.status}.[/green]")
            continue

        console.print(f"\n[bold]Executing {task.id}[/bold] with [bold]{normalized_executor}[/bold]...")
        executed_task_ids.append(task.id)
        if profile:
            run_command.run(task.id, executor=normalized_executor, profile=profile, project_root=root)
        else:
            run_command.run(task.id, executor=normalized_executor, project_root=root)

        latest = {item.id: item for item in _load_tasks(root)}.get(task.id)
        latest_status = latest.status if latest else "missing"
        if latest_status not in {"verified", "done"}:
            failed.append(f"{task.id} ({latest_status})")
            if latest_status != "blocked" or not continue_on_blocked:
                console.print(f"[red]Stopping because {task.id} ended as {latest_status}.[/red]")
                break

    if not executed_task_ids:
        failed.append("all planned tasks were already completed before execution")

    if not failed:
        _record_project_done(root)
    else:
        _record_project_blocked(root)

    console.print("\n[bold]Final DevCouncil report[/bold]")
    report_command.report(
        SimpleNamespace(invoked_subcommand=None),  # type: ignore[arg-type]
        planning_only=False,
        json_format=json_report,
        github=False,
        github_pr_comment=False,
        gitlab_pr_comment=False,
        project_root=root,
    )
    if report_file is not None:
        output = _render_final_report(root, json_report=json_report)
        written = _write_report_file(root, report_file, output)
        console.print(f"[green]Final report written to {written}[/green]")

    if failed:
        console.print(f"\n[red]Unfinished task(s): {', '.join(failed)}[/red]")
        raise typer.Exit(code=1)
