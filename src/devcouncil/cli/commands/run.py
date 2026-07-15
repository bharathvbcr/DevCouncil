import logging
import typer
from rich.console import Console
from pathlib import Path
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository
from devcouncil.executors.mini_swe import MiniSWEExecutor
from devcouncil.executors.openhands import OpenHandsExecutor
from devcouncil.executors.coding_cli import CodingCliExecutor
from devcouncil.executors.agent_registry import (
    AGENT_ALIASES,
    BUILTIN_CODING_EXECUTOR_NAMES,
    list_run_executor_names,
    load_cli_agent_specs,
)
from devcouncil.executors.native.agent import NativeAgent
from devcouncil.llm.provider import create_provider, validate_model_provider
from devcouncil.llm.router import ModelRouter
from devcouncil.app.config import load_config, get_api_key
from devcouncil.domain.evidence import CommandResult, DiffEvidence, TestEvidence
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.storage.repositories import GapRepository, EvidenceRepository, StateRepository
from devcouncil.verification.verifier import Verifier
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.cli.commands.init import initialize_project
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)
CODING_EXECUTOR_ALIASES = {name: name for name in BUILTIN_CODING_EXECUTOR_NAMES} | AGENT_ALIASES

CODING_EXECUTORS = set(CODING_EXECUTOR_ALIASES.keys())


def _custom_cli_agents(project_root: Path) -> set[str]:
    specs = load_cli_agent_specs(project_root)
    return {name for name, spec in specs.items() if not spec.built_in}

def _current_changed_files(project_root: Path = Path(".")) -> list[str]:
    from devcouncil.verification.verifier import Verifier

    return Verifier(project_root).get_changed_files()


def _verify_executor_output_if_present(
    session,
    task,
    reqs,
    *,
    root: Path,
    executor_label: str,
    cli_client: str | None = None,
    cli_executor=None,
) -> bool:
    """Run verification when the executor failed but left implementable changes."""
    if not _current_changed_files(root):
        return False
    console.print(
        f"[yellow]{executor_label} failed, but the workspace has changes — "
        f"verifying {task.id} against them anyway.[/yellow]"
    )
    verified = _verify_after_execution(
        session, task, reqs, router=_build_verification_router(root), project_root=root
    )
    _record_project_phase(
        session,
        ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
    )
    TaskRepository(session).save(task)
    if cli_client:
        _record_agent_verification(
            root,
            task.id,
            cli_client,
            getattr(cli_executor, "last_run_id", None) if cli_executor else None,
            verified,
        )
        _run_live_review_after_execution(root, cli_client, task.id)
    _log_exec_outcome(executor_label, task.id, verified=verified)
    if verified:
        console.print(f"\n[green]{executor_label} left verifiable work; {task.id} verified.[/green]")
    else:
        console.print(
            f"\n[yellow]{executor_label} left changes, but {task.id} is blocked by verification gaps.[/yellow]"
        )
    return True

def _capture_after_patch(task_id: str, project_root: Path = Path(".")):
    """Capture the diff after task execution for use by rollback."""
    try:
        from devcouncil.execution.checkpoints import CheckpointService

        CheckpointService(project_root).create_after(task_id)
    except Exception as e:
        # Non-critical — don't block execution, but a missing after checkpoint breaks rollback.
        logger.warning("Failed to capture after checkpoint for %s: %s", task_id, e)

def _capture_before_snapshot(task_id: str, project_root: Path = Path(".")):
    from devcouncil.execution.checkpoints import CheckpointService

    CheckpointService(project_root).create_before(task_id)

def _record_project_phase(session, phase: ProjectPhase):
    StateRepository(session).record_phase(phase.value)

def _verify_after_execution(
    session,
    task: Task,
    reqs: list[Requirement],
    router=None,
    project_root: Path = Path("."),
) -> bool:
    """Run deterministic verification after an automated executor finishes."""
    import asyncio

    logger.info("Verifying task %s (router=%s)", task.id, "yes" if router else "no")
    log_step(
        "run/verify: verifying executor output",
        project_root=project_root,
        task_id=task.id,
    )
    verifier = Verifier(project_root, router=router)
    gaps, evidence = asyncio.run(verifier.verify_task(task, reqs))
    blocking = [g for g in gaps if g.blocking]
    logger.info(
        "Verification of %s: %d gap(s) (%d blocking), %d evidence item(s)",
        task.id, len(gaps), len(blocking), len(evidence),
    )

    gap_repo = GapRepository(session)
    evidence_repo = EvidenceRepository(session)
    gap_repo.delete_for_task(task.id)
    evidence_repo.delete_for_task(task.id)

    for gap in gaps:
        gap_repo.save(gap)

    for ev in evidence:
        if isinstance(ev, CommandResult):
            evidence_repo.save_command_result(task.id, ev)
        elif isinstance(ev, DiffEvidence):
            evidence_repo.save_diff_evidence(ev)
        elif isinstance(ev, TestEvidence):
            evidence_repo.save_test_evidence(ev, task.id)

    task.status = "blocked" if any(g.blocking for g in gaps) else "verified"
    return task.status == "verified"


def _build_verification_router(project_root: Path):
    """Best-effort ``ModelRouter`` for LLM-backed verification after a coding-agent run.

    Without a router the ``Verifier`` runs deterministic checks only (no
    ``implementation_reviewer`` review, no acceptance-criterion compilation). The native
    executor already builds a router to *run* the agent and reuses it for verification;
    CLI coding agents (claude, codex, …) don't need one to execute, so they previously
    verified without the LLM review at all. Build one here so the review gate guides and
    monitors execution for those agents too. Per-role provider config means these review
    roles can run on a different provider than planning (e.g. local Ollama).

    Returns ``None`` when no provider/API key is configured so verification degrades to
    deterministic-only instead of erroring — the LLM review is an enhancement, not a
    hard requirement of running a task.
    """
    try:
        config = load_config(project_root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, project_root)
        provider = create_provider(
            config.models.provider, api_key, project_root=project_root, provider_prefs=config.provider
        )
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        return ModelRouter(provider, role_config, project_root=project_root)
    except Exception:
        return None


def _run_live_review_after_execution(project_root: Path, client: str, task_id: str | None) -> None:
    """Critique the coding agent's latest turn with the ``live_reviewer`` role.

    This is what makes live review actually fire during ``dev e2e``/``dev run`` (previously
    it only ran via ``dev watch``). It produces an advisory critique card — it does NOT
    gate the task; the deterministic/LLM verifier already does that. The card feeds the
    final report's live-review summary and is routed by per-role config (e.g. a local
    Ollama ``live_reviewer`` while planning runs on OpenRouter).

    The transcript is resolved the same way ``dev watch`` does — the client's NATIVE
    session log (e.g. claude's projects JSONL), discovered for this project root — not the
    executor's streamed ``transcript.txt`` (which only exists with --stream and isn't the
    structured turn format ``latest_assistant_turn`` parses).

    Best-effort and opt-outable: skipped when ``integrations.live_review.enabled`` is
    false, when no transcript is found, or on any error — a live-review hiccup must never
    fail the run.
    """
    import asyncio

    try:
        if not load_config(project_root).integrations.live_review.enabled:
            return
        from devcouncil.cli.commands.watch import (
            _resolve_transcript,
            _review_turn,
            _save_card_once,
            _log_card_reviewed,
        )
        from devcouncil.live.transcripts import latest_assistant_turn

        transcript = _resolve_transcript(project_root, client, latest=True, task_id=task_id)
        if transcript is None or not transcript.exists():
            return
        turn = latest_assistant_turn(transcript, client=client)
        if turn is None:
            return
        card = asyncio.run(_review_turn(turn, project_root, client, use_llm=True, task_id=task_id))
        card = card.model_copy(update={"task_id": task_id, "blocks_gate": False})
        saved_path, duplicate = _save_card_once(project_root, card, persist=True, force=False)
        if saved_path:
            _log_card_reviewed(project_root, card, saved_path, duplicate=duplicate, source="e2e")
            console.print(f"[dim]Live review ({card.verdict}): {saved_path}[/dim]")
    except Exception as exc:  # noqa: BLE001 - advisory; never fail the run on a review hiccup
        console.print(f"[yellow]Live review skipped: {exc}[/yellow]")


def _log_exec_outcome(executor: str, task_id: str, *, verified: bool) -> None:
    """Log a non-coding-CLI executor's post-verification outcome (verified vs blocked),
    so standalone ``dev run`` has the same flow-decision trail as ``dev go``."""
    if verified:
        logger.info("%s finished and %s verified", executor, task_id)
    else:
        logger.warning("%s finished but %s blocked by verification gaps", executor, task_id)


def _record_agent_verification(project_root: Path, task_id: str, executor: str, run_id: str | None, verified: bool) -> None:
    TraceLogger(project_root).log_event(
        "agent_run_verified",
        {"agent": executor, "verified": verified},
        run_id=run_id,
        task_id=task_id,
        summary=f"{executor} verification {'passed' if verified else 'blocked'} for {task_id}",
    )

def run(
    task_id: str = typer.Argument(..., help="ID of the task to run"),
    executor: str = typer.Option(
        "manual",
        "--executor",
        "-e",
        help=(
            "Executor to use (manual, mini, openhands, native-preview, claude-sdk, "
            "codex, gemini, claude, opencode, antigravity, warp, cursor, grok, aider, "
            "copilot, goose, amp, qwen, crush, or a configured agent)"
        ),
    ),
    profile: str | None = typer.Option(None, "--profile", help="CLI-agent execution profile: default, yolo, prod, or a configured profile."),
    stream: bool = typer.Option(
        False,
        "--stream",
        help="Stream coding CLI stdout/stderr live (also enabled by execution.stream_cli_output).",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Execute a specific task.
    """
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev run: task=%s executor=%s profile=%s stream=%s", task_id, executor, profile, stream)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        return

    with log_stage("run", project_root=root, task_id=task_id, executor=executor):
        _run_task_body(root, task_id, executor, profile, stream, db)


def _run_task_body(root, task_id, executor, profile, stream, db):
    with db.get_session() as session:
        task_repo = TaskRepository(session)
        task = task_repo.get_by_id(task_id)
        if not task:
            console.print(f"[red]Task {task_id} not found.[/red]")
            return

        from devcouncil.gating.policy import GatePolicy
        gate_policy = GatePolicy()
        gate_result = gate_policy.check_task_ready(task, root)
        if not gate_result.passed:
            logger.warning(
                "Task %s failed readiness gate: %s",
                task_id, "; ".join(g.description for g in gate_result.gaps if g.blocking),
            )
            console.print(f"[red]Task {task_id} is not ready for execution.[/red]")
            for gap in gate_result.gaps:
                if gap.blocking:
                    console.print(f" - [red][BLOCKING][/red] {gap.description} (Fix: {gap.recommended_fix})")
            return

        console.print(f"Running task [bold]{task_id}[/bold] using [bold]{executor}[/bold] executor...")

        log_step("run/1: creating git checkpoint", project_root=root, task_id=task_id)
        # 1. Create Git checkpoint
        try:
            from devcouncil.execution.checkpoints import CheckpointService

            result = CheckpointService(root).create_before(task_id)
            if result.patch_path:
                console.print(f"Created git checkpoint at {result.patch_path}")
            elif result.git_ref_created:
                console.print(f"Created git checkpoint ref {result.ref}")
        except Exception as e:
            console.print(f"[yellow]Warning: Failed to create git checkpoint: {e}[/yellow]")

        executor = executor.strip().lower().replace("_", "-")
        if executor == "claude-sdk":
            # Preflight the optional SDK BEFORE mutating any state: previously a
            # missing claude-agent-sdk was only discovered inside run_task, after
            # planning had already spent its budget — every such run failed at the
            # finish line with "SDK is not installed" (observed repeatedly in logs).
            # Degrade to the Claude Code CLI executor when it is available (same
            # agent, hook-based gating instead of in-process gating); otherwise
            # fail fast with the actionable install hint.
            import importlib.util
            import shutil as _shutil

            if importlib.util.find_spec("claude_agent_sdk") is None:
                if _shutil.which("claude"):
                    logger.warning(
                        "claude-agent-sdk is not installed; falling back to the "
                        "'claude' CLI executor for %s (install claude-agent-sdk "
                        "to restore in-process tool gating).", task_id,
                    )
                    console.print(
                        "[yellow]claude-agent-sdk is not installed — falling back to the "
                        "'claude' CLI executor. Install it with "
                        "`pip install claude-agent-sdk` to use --executor claude-sdk.[/yellow]"
                    )
                    executor = "claude"
                else:
                    logger.error("claude-sdk executor unavailable for %s: claude-agent-sdk not installed", task_id)
                    console.print(
                        "[red]The Claude Agent SDK is not installed and no `claude` CLI was found. "
                        "Install the SDK with `pip install claude-agent-sdk`, or use another "
                        "executor (e.g. --executor claude).[/red]"
                    )
                    return
        custom_agents = _custom_cli_agents(root)
        if executor not in CODING_EXECUTORS and executor not in custom_agents:
            ignored = [flag for flag, value in (("--profile", profile), ("--stream", stream)) if value]
            if ignored:
                console.print(
                    f"[yellow]{' and '.join(ignored)} only apply to coding CLI executors and are ignored for '{executor}'.[/yellow]"
                )
        if executor == "manual":
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            task.status = "running"
            task_repo.save(task)
            logger.info("%s marked RUNNING for manual sidecar execution", task_id)
            console.print(f"\n[green]Task {task_id} is now marked as RUNNING.[/green]")
            console.print("Use 'dev prompt TASK-ID' to get the prompt for this task.")
            console.print("When finished, use 'dev verify TASK-ID' to check the results.")
        elif executor in CODING_EXECUTORS or executor in custom_agents:
            log_step(
                f"run/2: executing with {executor}",
                project_root=root,
                task_id=task_id,
                executor=executor,
            )
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            cli_client = CODING_EXECUTOR_ALIASES.get(executor, executor)
            cli_executor = CodingCliExecutor(root, cli_client, profile=profile, stream_output=stream or None)
            exec_result = cli_executor.run_task(task, reqs)
            _capture_after_patch(task_id, root)
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(
                    session, task, reqs, router=_build_verification_router(root), project_root=root
                )
                _record_agent_verification(root, task.id, cli_client, getattr(cli_executor, "last_run_id", None), verified)
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                run_id = getattr(cli_executor, "last_run_id", None)
                transcript_path = getattr(cli_executor, "last_transcript_path", None)
                if run_id:
                    run_dir = root / ".devcouncil" / "runs" / run_id
                    console.print(f"Run artifacts: [dim]{run_dir}[/dim]")
                    if (run_dir / "run.log").exists():
                        console.print(f"Run log: [dim]dev logs tail --run {run_id}[/dim]")
                    if transcript_path:
                        console.print(f"Transcript: [dim]{transcript_path}[/dim]")
                # Live review (advisory): critique the agent's turn with the live_reviewer
                # role so monitoring actually happens during execution, not only via watch.
                _run_live_review_after_execution(root, cli_client, task.id)
                _log_exec_outcome(executor, task_id, verified=verified)
                if verified:
                    console.print(f"\n[green]{executor.upper()} finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]{executor.upper()} finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                logger.error("%s failed to start or execute for %s: %s", executor, task_id, exec_result.message)
                console.print(f"\n[red]{executor.upper()} failed to start or execute: {exec_result.message}[/red]")
                if not _verify_executor_output_if_present(
                    session,
                    task,
                    reqs,
                    root=root,
                    executor_label=executor.upper(),
                    cli_client=cli_client,
                    cli_executor=cli_executor,
                ):
                    task_repo.save(task)
        elif executor == "mini":
            log_step("run/2: executing with mini-SWE-agent", project_root=root, task_id=task_id)
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            mini_executor = MiniSWEExecutor(root)
            exec_result = mini_executor.run_task(task, reqs)
            _capture_after_patch(task_id, root)
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(
                    session, task, reqs, router=_build_verification_router(root), project_root=root
                )
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                _log_exec_outcome("mini-SWE-agent", task_id, verified=verified)
                if verified:
                    console.print(f"\n[green]mini-SWE-agent finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]mini-SWE-agent finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                logger.error("mini-SWE-agent failed to start or execute for %s", task_id)
                console.print("\n[red]mini-SWE-agent failed to start or execute.[/red]")
        elif executor == "openhands":
            log_step("run/2: executing with OpenHands", project_root=root, task_id=task_id)
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            oh_executor = OpenHandsExecutor(root)
            exec_result = oh_executor.run_task(task, reqs)
            _capture_after_patch(task_id, root)
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(
                    session, task, reqs, router=_build_verification_router(root), project_root=root
                )
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                _log_exec_outcome("OpenHands", task_id, verified=verified)
                if verified:
                    console.print(f"\n[green]OpenHands finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]OpenHands finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                logger.error("OpenHands failed to start or execute for %s", task_id)
                console.print("\n[red]OpenHands failed to start or execute.[/red]")
        elif executor == "claude-sdk":
            log_step("run/2: executing with claude-sdk", project_root=root, task_id=task_id)
            # In-process Claude Agent SDK executor: every tool call is gated live against
            # this task's scope, so out-of-scope writes/commands are denied before they land.
            from devcouncil.executors.claude_sdk import ClaudeSdkExecutor
            from devcouncil.executors.agent_registry import load_agent_profiles

            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            # Honor --profile for model and env overrides (e.g. ANTHROPIC_BASE_URL to
            # target an alternative Anthropic-compatible endpoint). permission_mode is
            # deliberately NOT taken from the profile: the SDK executor's containment
            # relies on "default" routing every tool call through can_use_tool; a
            # profile's acceptEdits/auto would silently bypass the live scope gate.
            sdk_profile = load_agent_profiles(root).get(profile or "default")
            sdk_executor = ClaudeSdkExecutor(
                root,
                active_task=task,
                model=(sdk_profile.model if sdk_profile else None),
                advisor_model=(sdk_profile.advisor_model if sdk_profile else None),
                env=(dict(sdk_profile.env) if sdk_profile and sdk_profile.env else None),
            )
            exec_result = sdk_executor.run_task(task, reqs)
            _capture_after_patch(task_id, root)
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(
                    session, task, reqs, router=_build_verification_router(root), project_root=root
                )
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                _run_live_review_after_execution(root, "claude", task.id)
                _log_exec_outcome("claude-sdk", task_id, verified=verified)
                if verified:
                    console.print(f"\n[green]claude-sdk finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]claude-sdk finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                logger.error("claude-sdk failed to start or execute for %s: %s", task_id, exec_result.message)
                console.print(f"\n[red]claude-sdk failed to start or execute: {exec_result.message}[/red]")
        elif executor in {"native", "native-preview"}:
            log_step("run/2: executing with native agent", project_root=root, task_id=task_id)
            # Load config for model routing and permissions
            try:
                config = load_config(root)
                validate_model_provider(config.models.provider)
                api_key = get_api_key(config.models.provider, root)
            except (FileNotFoundError, ValueError) as e:
                logger.error("Native executor cannot start for %s: %s", task_id, e)
                console.print(f"[red]{e}[/red]")
                return
            
            provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)
            role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
            router = ModelRouter(provider, role_config, project_root=root)
            
            # Setup Permission System
            from devcouncil.execution.permissions import PermissionPolicy, PermissionManager
            from devcouncil.execution.task_runner import TaskRunner
            
            # Populate policy from config commands
            allowed_cmds = config.commands.test + config.commands.lint + config.commands.typecheck
            permission_policy = PermissionPolicy(
                allowed_shell_commands=allowed_cmds,
            )
            perm_manager = PermissionManager(permission_policy, root)
            task_runner = TaskRunner(root, perm_manager)
            
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            agent = NativeAgent(router, task_runner)
            exec_result = agent.run_task(task, reqs)
            _capture_after_patch(task_id, root)
            
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(session, task, reqs, router=router, project_root=root)
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                _log_exec_outcome("Native agent", task_id, verified=verified)
                if verified:
                    console.print(f"\n[green]Native agent finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]Native agent finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                logger.error("Native agent failed during execution for %s", task_id)
                console.print("\n[red]Native agent failed during execution.[/red]")
        else:
            available = ", ".join(list_run_executor_names(root))
            logger.error("Executor %r not recognized; available: %s", executor, available)
            console.print(f"[red]Unknown executor {executor!r}.[/red]")
            console.print(f"[dim]Available executors: {available}[/dim]")
        log_step("run/complete", project_root=root, task_id=task_id, executor=executor, trace=True)
