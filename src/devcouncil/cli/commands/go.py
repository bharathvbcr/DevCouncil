import asyncio
import hashlib
import logging
import subprocess
from pathlib import Path
from types import SimpleNamespace

import typer
from rich.console import Console

from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.telemetry.logging_setup import set_log_dir

from devcouncil.app.config import load_config
from devcouncil.cli.commands import plan as plan_command
from devcouncil.cli.commands import report as report_command
from devcouncil.cli.commands import run as run_command
from devcouncil.cli.commands import verify as verify_command
from devcouncil.cli.commands.init import initialize_project
from devcouncil.executors.agent_registry import (
    AGENT_ALIASES,
    BUILTIN_CODING_EXECUTOR_NAMES,
    load_cli_agent_specs,
    resolve_automated_executor,
)
from devcouncil.integrations.github_intent import resolve_goal_intent
from devcouncil.llm.provider import ProviderRequestError
from devcouncil.llm.router import StructuredOutputError
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, GapRepository, StateRepository, TaskRepository
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.gating.policy import topological_order
from devcouncil.live.summary import live_review_summary
from devcouncil.reporting.report_builder import ReportBuilder


console = Console()
logger = logging.getLogger(__name__)

SUPPORTED_EXECUTORS = {
    *BUILTIN_CODING_EXECUTOR_NAMES,
    "native",
    "native-preview",
    "mini",
    "openhands",
}
SUPPORTED_EXECUTORS.update(AGENT_ALIASES)

AGENT_REPORT_FILE = Path(".devcouncil/reports/latest.json")


def _normalize_executor(executor: str) -> str:
    return executor.strip().lower().replace("_", "-")


def _is_git_repo(root: Path) -> bool:
    """True when ``root`` is inside a git work tree.

    The reconciliation pass re-verifies tasks against the *committed integrated*
    state, which only exists when ``_commit_task_changes`` could actually commit —
    i.e. inside a git repo. Without git there is nothing to reconcile (each task was
    already verified in isolation during execution), and re-scanning a dirty,
    non-git tree would spuriously flag unrelated files as orphan diffs.
    """
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--is-inside-work-tree"],
            cwd=root, capture_output=True, text=True,
        )
    except Exception:
        return False
    return result.returncode == 0 and result.stdout.strip() == "true"


def _custom_cli_agents(root: Path) -> set[str]:
    specs = load_cli_agent_specs(root)
    return {name for name, spec in specs.items() if not spec.built_in}


def _load_tasks(root: Path):
    db = get_db(root)
    if not db:
        return []
    with db.get_session() as session:
        return TaskRepository(session).get_all()


def _task_status(root: Path, task_id: str) -> str:
    latest = {item.id: item for item in _load_tasks(root)}.get(task_id)
    return latest.status if latest else "missing"


def _max_repair_attempts(root: Path) -> int:
    """How many self-repair attempts `dev go` may make per task (0 disables the loop)."""
    try:
        return max(0, int(load_config(root).execution.max_repair_attempts))
    except Exception:
        return 0


def _blocking_gap_signature(root: Path, task_id: str) -> str:
    """Fingerprint of a task's current blocking gaps, for no-progress detection.

    If a repair attempt reproduces the exact same blocking gaps as the previous one,
    the agent is stuck — we abort rather than burn the rest of the budget repeating a
    fix that does not move the gate.
    """
    db = get_db(root)
    if not db:
        return ""
    with db.get_session() as session:
        gaps = GapRepository(session).get_blocking_for_task(task_id)
    key = "\n".join(sorted(f"{g.gap_type}:{g.description}" for g in gaps))
    return hashlib.sha1(key.encode("utf-8")).hexdigest() if key else ""


def _remediable_incomplete_signature(root: Path, task_id: str) -> str:
    """Fingerprint of a task's remediable "incomplete" gaps (unproven acceptance criteria
    the executor could still prove), for driving and no-progress-checking the repair loop
    when the task verified without a hard block but isn't actually done."""
    from devcouncil.planning.correction_manifest import remediable_incomplete_gaps

    db = get_db(root)
    if not db:
        return ""
    with db.get_session() as session:
        gaps = remediable_incomplete_gaps(GapRepository(session).get_for_task(task_id))
    key = "\n".join(sorted(f"{g.gap_type}:{g.description}" for g in gaps))
    return hashlib.sha1(key.encode("utf-8")).hexdigest() if key else ""


def _build_repair_service(root: Path):
    """Best-effort LLM repair service used to sharpen the correction manifest's root
    cause. Returns None when no provider key is configured — the manifest still has a
    deterministic, task-scoped fallback (allowed files, commands, forbidden changes)."""
    try:
        from devcouncil.app.config import get_api_key
        from devcouncil.llm.provider import create_provider, validate_model_provider
        from devcouncil.llm.router import ModelRouter
        from devcouncil.planning.repair_service import RepairService

        config = load_config(root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
        provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        return RepairService(ModelRouter(provider, role_config, project_root=root))
    except Exception:
        return None


def _execute_task_with_repair(
    root: Path,
    task,
    *,
    executor: str,
    profile: str | None,
    stream: bool,
    max_repairs: int,
    repair_service,
    config=None,
) -> tuple[str, int]:
    """Run a task, then self-repair in a bounded loop until it verifies or the budget
    is exhausted. Returns ``(final_status, repair_attempts_used)``.

    Each repair attempt writes a correction manifest (which the coding-CLI executor
    folds into its prompt) and re-runs the executor. Between attempts the prior work
    is committed so the readiness gate's clean-tree requirement holds and the next
    attempt builds on it; verification still recognises the committed work via the
    task's checkpoint. The loop stops early when an attempt makes no progress (the
    same blocking gaps reappear) so it never spins on an unfixable gate.

    When the task ultimately verifies, the intermediate ``[blocked]`` commits made
    between attempts are squashed into a single verified commit (see
    :func:`_squash_repair_commits`) so failed attempts don't pollute git history. The
    squash preserves the task's checkpoint refs, so the verifier's empty-diff guard
    and ``dev rollback`` keep working.
    """
    from devcouncil.planning.correction_manifest import write_correction_manifest

    def _run_once() -> None:
        # An executor that raises (e.g. an experimental native agent hitting a
        # StructuredOutputError, or a CLI crash) must not abort the whole `dev go`
        # run — record it and let the loop/report treat the task as blocked.
        try:
            run_command.run(task.id, executor=executor, profile=profile, stream=stream, project_root=root)
        except Exception as exc:  # noqa: BLE001 - executor faults are non-fatal to the run
            console.print(f"[red]{task.id}: executor '{executor}' errored: {exc}[/red]")

    # HEAD before this task makes any commit, captured lazily right before the first
    # intermediate commit. On a successful repair we `git reset --soft` back to here
    # so only one verified commit remains (squashing the [blocked] attempts).
    squash_base: str | None = None
    intermediate_commits = 0

    _run_once()
    status = _task_status(root, task.id)
    logger.info("Initial run of %s finished as %s (max_repairs=%d)", task.id, status, max_repairs)

    attempt = 0
    last_signature: str | None = None
    while attempt < max_repairs:
        blocked = status not in {"verified", "done"}
        # When the task is blocked, repair against its blocking gaps. When it "verified"
        # without a hard block but is still INCOMPLETE (an acceptance criterion the
        # executor could prove has no passing evidence), keep repairing too — otherwise
        # arm B stalls one proof short of done (the eval_rpn 5/7-incomplete case).
        signature = (
            _blocking_gap_signature(root, task.id) if blocked
            else _remediable_incomplete_signature(root, task.id)
        )
        if not signature:
            # Nothing concrete to repair: truly done, or blocked with no recorded gaps
            # (e.g. the executor failed to start).
            break
        if signature == last_signature:
            logger.warning("%s: repair made no progress (identical gaps) after attempt %d; stopping loop", task.id, attempt)
            console.print(
                f"[yellow]{task.id}: repair made no progress (identical blocking gaps); "
                "stopping the self-repair loop.[/yellow]"
            )
            break
        last_signature = signature

        # Record where history started before the first failed-attempt commit, so the
        # squash collapses exactly this task's intermediate commits and nothing earlier.
        if squash_base is None:
            squash_base = _current_head(root)

        # Commit the prior attempt so the next run starts from a clean tree (the
        # readiness gate requires it); the committed work stays visible to verify.
        # Marked [blocked] and squashed away later when the task verifies.
        if _commit_task_changes(root, task.id, status):
            intermediate_commits += 1

        manifest_path = write_correction_manifest(
            root, task.id, repair_service=repair_service, config=config, include_incomplete=True
        )
        if manifest_path is None:
            break
        attempt += 1
        logger.info("Self-repair attempt %d/%d for %s (was %s); manifest=%s", attempt, max_repairs, task.id, status, manifest_path)
        console.print(
            f"\n[bold]Self-repair attempt {attempt}/{max_repairs}[/bold] for "
            f"[bold]{task.id}[/bold] (was {status})..."
        )
        _run_once()
        status = _task_status(root, task.id)
        logger.info("After repair attempt %d, %s is now %s", attempt, task.id, status)

    if status not in {"verified", "done"} and attempt >= max_repairs and max_repairs > 0:
        logger.warning("%s: gave up after %d repair attempt(s); still %s", task.id, attempt, status)
        console.print(
            f"[yellow]{task.id}: gave up after {attempt} repair attempt(s); still {status}.[/yellow]"
        )

    # On success, squash the [blocked] attempt commits into one verified commit so the
    # user's history isn't littered with failed attempts. Only do this when the task
    # actually verified — a still-blocked task keeps its attempt commits so the work
    # isn't lost and the final reconciliation pass still sees committed changes.
    if status in {"verified", "done"} and squash_base and intermediate_commits:
        if _squash_repair_commits(root, task.id, squash_base, status):
            logger.info("Squashed %d blocked attempt commit(s) for %s into one verified commit", intermediate_commits, task.id)
            console.print(
                f"[dim]Squashed {intermediate_commits} blocked attempt commit(s) for "
                f"{task.id} into one verified commit.[/dim]"
            )

    return status, attempt


def _current_head(root: Path) -> str | None:
    """Resolve the current HEAD commit, or None when there is no commit / no git."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=root, capture_output=True, text=True,
        )
    except Exception:
        return None
    if result.returncode != 0:
        return None
    head = result.stdout.strip()
    return head or None


def _squash_repair_commits(root: Path, task_id: str, base: str, status: str) -> bool:
    """Collapse this task's intermediate ``[blocked]`` commits into one verified commit.

    Soft-resets HEAD back to ``base`` (the commit before the first failed-attempt
    commit) and re-commits the combined tree once. A soft reset moves only the branch
    pointer: the index and working tree are untouched and every prior commit object
    stays alive while a ref points at it. The task's checkpoint refs
    (``refs/devcouncil/tasks/<id>/before|after``) are independent named refs, so they
    still resolve after the squash — which keeps the verifier's empty-diff guard
    (``git diff <before_ref>``) and ``dev rollback`` (``git diff <before> <after>``)
    working against the same commit objects as before.

    Best-effort: any failure leaves the (already-committed) intermediate history in
    place rather than risking the tree, and returns False. Returns True on success.
    """
    try:
        # Guard: base must be a real ancestor we can reset to, and there must be
        # commits since it to squash. If base == HEAD there is nothing to do.
        head = _current_head(root)
        if not head or head == base:
            return False
        base_ok = subprocess.run(
            ["git", "rev-parse", "--verify", f"{base}^{{commit}}"],
            cwd=root, capture_output=True, text=True,
        )
        if base_ok.returncode != 0:
            return False
        # Soft reset keeps the working tree + index exactly as-is; only the branch
        # pointer moves back to base, so the next commit captures the whole task.
        reset = subprocess.run(
            ["git", "reset", "--soft", base],
            cwd=root, capture_output=True, text=True,
        )
        if reset.returncode != 0:
            return False
        # Stage the FINAL (verified) attempt's still-uncommitted changes too, so they land
        # in this single squash commit. Without this they'd be committed separately by the
        # caller afterward, producing two commits for what the message calls "one verified
        # commit" (and leaving the squash to capture only the [blocked] diffs).
        add = subprocess.run(
            ["git", "add", "-A"],
            cwd=root, capture_output=True, text=True,
        )
        if add.returncode != 0:
            return False
        # Re-commit the squashed tree. There may be nothing staged if every attempt's
        # changes cancelled out (unlikely for a verified task) — tolerate that.
        commit = subprocess.run(
            [
                "git",
                "-c", "user.name=DevCouncil",
                "-c", "user.email=devcouncil@local",
                "commit", "--no-verify", "--allow-empty",
                "-m", f"devcouncil(e2e): {task_id} [{status}]",
            ],
            cwd=root, capture_output=True, text=True,
        )
        return commit.returncode == 0
    except Exception:
        return False


def _commit_task_changes(root: Path, task_id: str, status: str) -> bool:
    """Commit the working-tree changes a task produced.

    Sequential plans build on each other and the task-readiness gate requires a
    clean working tree, so without this each task after the first is blocked by
    the previous task's uncommitted changes. Commits are clearly attributed to
    DevCouncil (via ``-c`` so the user's git identity/config is never mutated)
    and can be squashed or reset afterwards. Returns True if a commit was made.
    """
    try:
        status_out = subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=root, capture_output=True, text=True,
        )
        if status_out.returncode != 0 or not status_out.stdout.strip():
            return False
        subprocess.run(["git", "add", "-A"], cwd=root, check=True, capture_output=True)
        commit = subprocess.run(
            [
                "git",
                "-c", "user.name=DevCouncil",
                "-c", "user.email=devcouncil@local",
                "commit", "--no-verify",
                "-m", f"devcouncil(e2e): {task_id} [{status}]",
            ],
            cwd=root, capture_output=True, text=True,
        )
        return commit.returncode == 0
    except Exception:
        return False


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
    goal: str = typer.Argument(..., help="Implementation goal to plan, execute, verify, and report. Also accepts a GitHub issue/PR reference (#142, owner/repo#142, or a github.com URL), whose title+body becomes the goal."),
    executor: str | None = typer.Option(
        None,
        "--executor",
        "-e",
        help="Automated executor to use. Defaults to execution.default_executor in .devcouncil/config.yaml.",
    ),
    dry_run: bool = typer.Option(False, "--dry-run", help="Use mock planning responses for local smoke testing."),
    quick: bool = typer.Option(
        False,
        "--quick",
        help="Rigor dial: skip the planning council (A/B debate, critique, rebuttal, arbitration) "
        "for a single spec + plan. Faster and cheaper; verification still gates every diff.",
    ),
    force: bool = typer.Option(
        False,
        "--force",
        "--yes",
        "-y",
        help="Proceed past unresolved planning gaps (critique findings, blocking questions) "
        "without manual approval. Verification still gates each task's actual diff.",
    ),
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
    stream: bool = typer.Option(
        False,
        "--stream",
        help="Stream coding CLI stdout/stderr live during execution (also enabled by execution.stream_cli_output).",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Run the full DevCouncil loop in one command.
    """
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    logger.info(
        "dev go starting: goal=%r executor=%s quick=%s force=%s root=%s",
        goal, executor, quick, force, root,
    )
    initialize_project(root, quiet=True)

    # A goal like "#142" or a GitHub issue/PR URL is a reference, not a spec —
    # expand it into the issue/PR title + body (the real intent) via the gh CLI.
    expanded_goal, intent_note = resolve_goal_intent(goal, root)
    if intent_note:
        console.print(f"[dim]{intent_note}[/dim]")
    goal = expanded_goal

    if agent:
        json_report = True
        if report_file is None:
            report_file = AGENT_REPORT_FILE

    normalized_executor = resolve_automated_executor(root, executor)
    command_label = _command_label(ctx)
    if normalized_executor == "manual":
        console.print(
            f"[red]`{command_label}` requires an automated executor. "
            "Set execution.default_executor in .devcouncil/config.yaml or install a coding CLI on PATH.[/red]"
        )
        raise typer.Exit(code=2)
    if executor is None and normalized_executor != "manual":
        console.print(
            f"[dim]Using automated executor:[/dim] [bold]{normalized_executor}[/bold] "
            "(from config or first coding CLI found on PATH)."
        )
    supported = SUPPORTED_EXECUTORS | _custom_cli_agents(root)
    if normalized_executor not in supported:
        console.print(
            f"[red]Unsupported executor for `{command_label}`: "
            f"{normalized_executor}. Supported: {', '.join(sorted(supported))}.[/red]"
        )
        raise typer.Exit(code=2)

    console.print(f"[bold]Planning goal:[/bold] {goal}")
    try:
        with log_stage("plan", project_root=root, quick=quick, dry_run=dry_run):
            planned_task_ids = asyncio.run(plan_command.run_plan_flow(goal, dry_run=dry_run, persist=True, project_root=root, quick=quick))
    except (ProviderRequestError, StructuredOutputError) as exc:
        plan_command.print_planning_error(exc)
        raise typer.Exit(code=1)

    task_ids = _unique_task_ids(planned_task_ids or [])
    # The planning council almost always raises advisory gaps (critique findings,
    # clarifying questions), so run_plan_flow returns no approved tasks and the
    # plan is left in AWAITING_USER_DECISIONS. For an automated one-command flow
    # that means there is nothing to run. With --force, approve the generated plan
    # anyway and proceed — verification still gates each task's actual diff.
    if not task_ids:
        if force:
            logger.info("No auto-approved tasks; force-approving generated plan past planning gaps")
            try:
                plan_command.approve(run_id=None, force=True, project_root=root)
            except SystemExit:
                pass
            tasks = _load_tasks(root)
            if tasks:
                console.print(
                    "[yellow]Proceeding past planning gaps via --force; "
                    "verification still gates each task.[/yellow]"
                )
        else:
            tasks = []
        if not tasks:
            logger.warning("Planning produced no approved tasks; aborting run")
            console.print("[red]Planning did not produce any approved tasks.[/red]")
            console.print(
                "Review gaps with [bold]dev status[/bold], then run [bold]dev approve[/bold] "
                "to accept the plan — or re-run with [bold]--force[/bold] to proceed past "
                "advisory planning gaps automatically."
            )
            raise typer.Exit(code=1)
    else:
        tasks, missing_task_ids = _load_tasks_by_id(root, task_ids)
        if missing_task_ids:
            console.print(f"[red]Planning returned task IDs that were not persisted: {', '.join(missing_task_ids)}[/red]")
            raise typer.Exit(code=1)
        if not tasks:
            console.print("[red]Planning did not produce any approved tasks.[/red]")
            raise typer.Exit(code=1)

    failed: list[str] = []
    executed_task_ids: list[str] = []
    # Automated executors can self-repair; manual sidecar mode cannot (a human drives
    # the edits), so the repair loop only applies to automated runs.
    max_repairs = _max_repair_attempts(root) if normalized_executor != "manual" else 0
    repair_service = _build_repair_service(root) if max_repairs else None
    # Load config once for the repair loop so the correction-manifest builder doesn't
    # reload it from disk on every repair attempt. Only needed when the loop is active;
    # the builder falls back to loading config itself if this is None.
    repair_config = load_config(root) if max_repairs else None
    # Run tasks in dependency order so a task never executes before the tasks it needs.
    tasks = topological_order(tasks)
    log_step(
        f"execution plan: {len(tasks)} task(s) in dependency order",
        project_root=root,
        order=[t.id for t in tasks],
    )
    completed_ids = {task.id for task in tasks if task.status in {"verified", "done"}}
    for task in tasks:
        if task.status in {"verified", "done"}:
            logger.info("Skipping %s; already %s", task.id, task.status)
            console.print(f"[green]Skipping {task.id}; already {task.status}.[/green]")
            completed_ids.add(task.id)
            continue

        # Don't run a task whose prerequisites didn't complete — it would fail for an
        # unrelated reason and (with the repair loop) burn its whole budget against an
        # unsatisfiable precondition. Skip it and surface why.
        unmet = [dep for dep in task.depends_on if dep not in completed_ids]
        if unmet:
            logger.warning("Skipping %s: upstream %s not completed", task.id, ", ".join(unmet))
            console.print(f"[yellow]Skipping {task.id}: upstream {', '.join(unmet)} not completed.[/yellow]")
            failed.append(f"{task.id} (skipped: upstream {', '.join(unmet)} unsatisfied)")
            continue

        console.print(f"\n[bold]Executing {task.id}[/bold] with [bold]{normalized_executor}[/bold]...")
        executed_task_ids.append(task.id)
        # Run, then self-repair in a bounded loop (closes the autonomous loop: the
        # one-shot executor no longer needs a human to run `dev repair` and re-run).
        with log_stage(
            "execute_task",
            project_root=root,
            task_id=task.id,
            executor=normalized_executor,
            max_repairs=max_repairs,
        ):
            latest_status, repairs_used = _execute_task_with_repair(
                root,
                task,
                executor=normalized_executor,
                profile=profile,
                stream=stream,
                max_repairs=max_repairs,
                repair_service=repair_service,
                config=repair_config,
            )
        log_step(
            f"task {task.id} finished as {latest_status}",
            project_root=root,
            task_id=task.id,
            repairs_used=repairs_used,
            trace=True,
        )

        # Commit whatever this task produced so the next task in the plan starts
        # from a clean tree — otherwise its readiness gate blocks on the dirty
        # tree and the whole multi-task plan stalls after task one.
        if _commit_task_changes(root, task.id, latest_status):
            note = f" after {repairs_used} repair attempt(s)" if repairs_used else ""
            logger.info("Committed %s changes (%s)%s", task.id, latest_status, note)
            console.print(f"[dim]Committed {task.id} changes ({latest_status}){note}.[/dim]")

        if latest_status in {"verified", "done"}:
            completed_ids.add(task.id)
        else:
            failed.append(f"{task.id} ({latest_status})")
            # --continue-on-blocked is a "run the whole plan, best effort" switch:
            # don't let one task that blocked or could not start halt the rest. The
            # final reconciliation pass judges the integrated result fairly.
            if not continue_on_blocked:
                logger.warning("Stopping run: %s ended as %s (no --continue-on-blocked)", task.id, latest_status)
                console.print(f"[red]Stopping because {task.id} ended as {latest_status}.[/red]")
                break
            logger.info("%s ended as %s; continuing to next task (--continue-on-blocked)", task.id, latest_status)
            console.print(f"[yellow]{task.id} ended as {latest_status}; continuing to the next task.[/yellow]")

    if not executed_task_ids:
        failed.append("all planned tasks were already completed before execution")

    # Final reconciliation: re-verify every task against the fully integrated,
    # committed state. Earlier tasks are verified before later tasks create shared
    # test files, so their gates can pass now even though they blocked mid-run.
    # The tree is clean here, so verification uses each task's committed checkpoint
    # diff to prove its acceptance criteria (rather than skipping on an empty diff and
    # wrongly blocking). Re-running the same diff is largely an LLM-cache hit, so this
    # refreshes statuses/gaps cheaply for an honest final report.
    if executed_task_ids and _is_git_repo(root):
        console.print("\n[bold]Reconciling verification against the final integrated state...[/bold]")
        log_step("reconcile: re-verifying against integrated state", project_root=root)
        try:
            verify_command.verify(task_id=None, sandbox="local", json_format=True, project_root=root)
        except typer.Exit:
            # Expected signal: verify() raises Exit(code=1) when any task is blocked,
            # but it has already persisted every task status before raising. The
            # reconciliation pass therefore completed — fall through to the reload so
            # blocked statuses are refreshed honestly. (Only real errors should skip.)
            pass
        except Exception as exc:  # pragma: no cover - reconciliation is best-effort
            console.print(f"[yellow]Reconciliation pass skipped: {exc}[/yellow]")
        reconciled = {item.id: item for item in _load_tasks(root)}
        # Rebuild from the FULL planned set, not just executed_task_ids: a task skipped
        # for an unmet dependency, or one reconciliation downgraded from done->blocked,
        # must still count as unfinished — otherwise `dev go` reports success while work
        # is incomplete.
        failed = []
        for planned in tasks:
            item = reconciled.get(planned.id)
            if not (item and item.status in {"verified", "done"}):
                status = item.status if item else "missing"
                failed.append(f"{planned.id} ({status})")

    if not failed:
        logger.info("dev go complete: all tasks finished")
        _record_project_done(root)
    else:
        logger.warning("dev go finished with %d unfinished task(s): %s", len(failed), ", ".join(failed))
        _record_project_blocked(root)

    log_step("generating final report", project_root=root)
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
