import typer
import asyncio
import datetime
import logging
import sys
from typing import Any
from rich.console import Console
from rich.panel import Panel
from rich.progress import Progress, SpinnerColumn, TextColumn
from pathlib import Path

from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import (
    GapRepository,
    PlanningStateRepository,
)
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.llm.provider import Provider, MockProvider, ProviderRequestError, build_role_model_config, create_provider, validate_model_provider
from devcouncil.llm.router import ModelRouter, StructuredOutputError
from devcouncil.planning.spec_service import SpecService
from devcouncil.planning.prompt_enhancer_service import PromptEnhancerService, save_active_prompt_enhancement
from devcouncil.planning.plan_service import PlanService, backfill_acceptance_criteria
from devcouncil.planning.plan_difficulty import apply_plan_difficulty
from devcouncil.planning.planned_files_reconcile import (
    expand_scope_with_dependents,
    reconcile_planned_files,
    repo_files_from_map,
)
from devcouncil.planning.question_conversion import convert_blocking_questions_to_assumptions
from devcouncil.planning.critique_service import CritiqueService
from devcouncil.planning.arbiter_service import ArbiterDecision, ArbiterService
from devcouncil.gating.policy import GatePolicy
from devcouncil.app.orchestrator import Orchestrator
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.app.config import ModelRoleConfig, load_config, get_api_key
from devcouncil.cli.commands.init import initialize_project
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

REQUIRED_PLANNING_ROLES = (
    "prompt_enhancer",
    "spec_writer",
    "planner_a",
    "planner_b",
    "critic_a",
    "critic_b",
    "arbiter",
)


def _decision_ids(items: list[Any]) -> set[str]:
    ids: set[str] = set()
    for item in items:
        if isinstance(item, str):
            ids.add(item)
        elif isinstance(item, dict):
            value = item.get("id") or item.get("finding_id")
            if value:
                ids.add(str(value))
    return ids


def _reconcile_findings(findings, decision):
    accepted_ids = set(decision.accepted_finding_ids)
    rejected_ids = _decision_ids(decision.rejected_finding_ids)
    reconciled = []
    for finding in findings:
        if finding.id in accepted_ids:
            reconciled.append(finding.model_copy(update={"status": "converted"}))
        elif finding.id in rejected_ids:
            reconciled.append(finding.model_copy(update={"status": "rejected"}))
        else:
            reconciled.append(finding)
    return reconciled


def _ensure_planning_roles(config) -> None:
    fallback = config.models.roles.get("spec_writer")
    if fallback is None and config.models.roles:
        fallback = next(iter(config.models.roles.values()))
    if fallback is None:
        try:
            provider_roles = build_role_model_config(config.models.provider)
            fallback = ModelRoleConfig(model=provider_roles["spec_writer"]["model"])
        except ValueError:
            fallback = ModelRoleConfig(model="unconfigured")

    for role in REQUIRED_PLANNING_ROLES:
        config.models.roles.setdefault(role, fallback.model_copy())


def _should_auto_convert_blocking_questions(config) -> bool:
    if not config.planning.auto_convert_blocking_questions_in_noninteractive:
        return False
    return not sys.stdin.isatty()


def _maybe_convert_blocking_questions(
    spec_output,
    config,
    console,
    *,
    orchestrator=None,
    artifact_path: Path | None = None,
):
    """In non-interactive mode, turn blocking questions into non-blocking assumptions."""
    if not _should_auto_convert_blocking_questions(config):
        return spec_output
    if not spec_output.blocking_questions:
        return spec_output

    question_count = len(spec_output.blocking_questions)
    assumptions, cleared = convert_blocking_questions_to_assumptions(
        spec_output.assumptions,
        spec_output.blocking_questions,
    )
    spec_output = spec_output.model_copy(
        update={"assumptions": assumptions, "blocking_questions": cleared},
    )
    console.print(
        f"[dim]Non-interactive mode: converted {question_count} blocking question(s) "
        "to open assumptions (will not block plan approval).[/dim]"
    )
    if orchestrator is not None and orchestrator.current_run is not None:
        orchestrator.save_run_artifact("requirements.json", spec_output.model_dump())
    elif artifact_path is not None:
        artifact_path.write_text(spec_output.model_dump_json(indent=2), encoding="utf-8")
    return spec_output


async def run_plan_flow(
    goal: str,
    requirements_only: bool = False,
    dry_run: bool = False,
    persist: bool = True,
    project_root: Path = Path("."),
    quick: bool = False,
):
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        return []

    # Load validated config
    config = load_config(root)
    _ensure_planning_roles(config)

    api_key = None
    if not dry_run:
        try:
            validate_model_provider(config.models.provider)
            api_key = get_api_key(config.models.provider, root)
        except ValueError as e:
            console.print(f"[red]{e}[/red]")
            return []

    orchestrator = Orchestrator(root, persist_state=persist)
    orchestrator.reset_state_machine(ProjectPhase.NEW)
    run_id = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-plan"
    await orchestrator.start_run(run_id, goal)

    provider: Provider
    if dry_run:
        # Override config models to be unique roles for mock mapping
        for role in REQUIRED_PLANNING_ROLES:
            config.models.roles[role].model = f"mock/{role}"

        provider = MockProvider()
        provider.responses = {
            "mock/prompt_enhancer": dump_json({
                "original_goal": goal,
                "enhanced_goal": (
                    f"Plan and implement {goal} using the mapped repository's "
                    "existing patterns, tests, and verification gates."
                ),
                "codebase_context": ["Use the repository map to target existing application and test structure."],
                "debate_focus": ["Compare minimal implementation scope against production-readiness concerns."],
                "constraints": [f"Do not broaden beyond: {goal}."]
            }),
            "mock/spec_writer": dump_json({
                "requirements": [{"id": "REQ-001", "title": "Mock Req", "description": "Desc", "priority": "high", "source": "user", "acceptance_criteria": []}],
                "assumptions": [],
                "blocking_questions": []
            }),
            "mock/planner_a": [
                dump_json({
                    "id": "PLAN-A", "rationale": "Simple", "tasks": [{"id": "TASK-001", "title": "Mock Task", "description": "Desc", "requirement_ids": ["REQ-001"], "acceptance_criterion_ids": [], "planned_files": [], "expected_tests": [], "allowed_commands": [], "status": "planned"}]
                }),
                dump_json({"rebuttals": []})
            ],
            "mock/planner_b": [
                dump_json({
                    "id": "PLAN-B", "rationale": "Robust", "tasks": [{"id": "TASK-001", "title": "Mock Task", "description": "Desc", "requirement_ids": ["REQ-001"], "acceptance_criterion_ids": [], "planned_files": [], "expected_tests": [], "allowed_commands": [], "status": "planned"}]
                }),
                dump_json({"rebuttals": []})
            ],
            "mock/critic_a": '{"findings": []}',
            "mock/critic_b": '{"findings": []}',
            "mock/arbiter": dump_json({
                "accepted_finding_ids": [], "rejected_finding_ids": [], 
                "final_requirements": [{"id": "REQ-001", "title": "Mock Req", "description": "Desc", "priority": "high", "source": "user", "acceptance_criteria": [{"id": "AC-1", "description": "Test it", "verification_method": "unit_test"}]}],
                "final_tasks": [{"id": "TASK-001", "title": "Mock Task", "description": "Desc", "requirement_ids": ["REQ-001"], "acceptance_criterion_ids": ["AC-1"], "planned_files": [{"path": "test.py", "reason": "logic", "allowed_change": "modify"}], "expected_tests": [], "allowed_commands": [], "status": "planned"}]
            }),
        }
        # Special case: PlanService calls use the same model names. 
        # I'll modify PlanService to use a slightly different role string if needed, 
        # but for Dry Run, let's just make the MockProvider return based on the schema requested.
    else:
        if api_key is None:
            console.print("[red]Missing API key for configured model provider.[/red]")
            return []
        provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)

    # Build role config after dry-run overrides so mocks are routed correctly.
    role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
    router = ModelRouter(provider, role_config, project_root=root)
    
    prompt_enhancer = PromptEnhancerService(router)
    spec_service = SpecService(router)
    plan_service = PlanService(router)
    critique_service = CritiqueService(router)
    arbiter_service = ArbiterService(router)
    mapper = RepoMapper(root)

    with log_stage("plan", project_root=root, run_id=run_id, goal=goal, quick=quick, dry_run=dry_run):
        return await _run_plan_body(
            root, goal, requirements_only, dry_run, persist, quick,
            orchestrator, run_id, db, config, router,
            prompt_enhancer, spec_service, plan_service, critique_service,
            arbiter_service, mapper,
        )


async def _run_plan_body(
    root, goal, requirements_only, dry_run, persist, quick,
    orchestrator, run_id, db, config, router,
    prompt_enhancer, spec_service, plan_service, critique_service,
    arbiter_service, mapper,
):
    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        transient=True,
    ) as progress:
        # 1. Repo Map
        log_step("plan/1: mapping repository", project_root=root, run_id=run_id, trace=True)
        progress.add_task(description="Mapping repository...", total=None)
        repo_map = mapper.map_repo(goal)
        repo_map_json = repo_map.model_dump_json(indent=2)
        # save_run_artifact re-serializes via json.dump, so pass the dict directly
        # instead of round-tripping the already-serialized JSON back through json.loads.
        orchestrator.save_run_artifact("repo_map.json", repo_map.model_dump(mode="json"))
        graph_context = CodeReviewGraphAdapter(root).get_context()
        if graph_context.available:
            orchestrator.save_run_artifact("code_review_graph_context.json", graph_context.model_dump())
        await orchestrator.transition_to(ProjectPhase.REPO_MAPPED)

        # 2. Codebase-specific prompt enhancement
        log_step("plan/2: enhancing prompt for codebase debate", project_root=root, run_id=run_id, trace=True)
        progress.add_task(description="Enhancing prompt for codebase debate...", total=None)
        prompt_enhancement = await prompt_enhancer.enhance_prompt(
            goal,
            repo_map_json,
            graph_context.model_dump_json() if graph_context.available else None,
            project_root=root,
        )
        debate_goal = prompt_enhancement.debate_prompt()
        orchestrator.save_run_artifact("prompt_enhancement.json", prompt_enhancement.model_dump())
        if prompt_enhancement.applied_skills:
            console.print(
                "[dim]Domain skills applied:[/dim] "
                + ", ".join(prompt_enhancement.applied_skills)
            )
        TraceLogger(root).log_event(
            "prompt_enhanced",
            {
                "original_goal": goal,
                "enhanced_goal": prompt_enhancement.enhanced_goal,
                "codebase_context_count": len(prompt_enhancement.codebase_context),
                "constraint_count": len(prompt_enhancement.constraints),
                "debate_focus_count": len(prompt_enhancement.debate_focus),
                "applied_skills": prompt_enhancement.applied_skills,
                "artifact": f".devcouncil/runs/{run_id}/prompt_enhancement.json",
            },
            run_id=run_id,
            summary="Prompt enhanced for codebase-specific debate.",
        )

        # 3. Spec / Requirements
        log_step("plan/3: generating requirements", project_root=root, run_id=run_id, trace=True)
        progress.add_task(description="Generating requirements...", total=None)
        spec_output = await spec_service.generate_spec(debate_goal, repo_map_json)
        orchestrator.save_run_artifact("requirements.json", spec_output.model_dump())
        await orchestrator.transition_to(ProjectPhase.REQUIREMENTS_DRAFTED)
        
        if requirements_only:
            console.print(Panel(f"Found {len(spec_output.requirements)} requirements.", title="Requirements Generated"))
            return []

        requirements_json = dump_json([r.model_dump() for r in spec_output.requirements])

        if quick:
            # Rigor dial: single pragmatic plan, no A/B debate, critique, rebuttal,
            # or arbitration. Spec requirements (with their acceptance criteria)
            # become the final requirements verbatim. This trades the council's
            # adversarial robustness for ~5 fewer model calls — the right setting
            # for small, well-scoped changes where verification (which still gates
            # every diff) is the real safety net, not planning debate.
            log_step("plan/4: generating single plan (quick mode)", project_root=root, run_id=run_id, trace=True)
            progress.add_task(description="Generating single plan (quick mode)...", total=None)
            plan_a = await plan_service.generate_plan(
                "planner_a", debate_goal, requirements_json, repo_map_json
            )
            orchestrator.save_run_artifact("plan_a.json", plan_a.model_dump())
            await orchestrator.transition_to(ProjectPhase.PLANS_GENERATED)

            decision = ArbiterDecision(
                accepted_finding_ids=[],
                rejected_finding_ids=[],
                final_requirements=spec_output.requirements,
                final_tasks=plan_a.tasks,
            )
            orchestrator.save_run_artifact("decision.json", decision.model_dump())
            # Walk through CRITIQUES_GENERATED (the only path to ARBITRATED) without
            # actually critiquing, so the rest of the lifecycle (approval, gates,
            # status, the report's phase) is identical to the full council flow.
            await orchestrator.transition_to(ProjectPhase.CRITIQUES_GENERATED)
            await orchestrator.transition_to(ProjectPhase.ARBITRATED)
            reconciled_findings = []
            final_tasks = [task.model_copy(update={"status": "planned"}) for task in decision.final_tasks]
        else:
            # 4. Independent Plans (run concurrently — they don't depend on each other)
            log_step("plan/4: generating Plans A (pragmatic) and B (robust)", project_root=root, run_id=run_id, trace=True)
            progress.add_task(description="Generating Plans A (Pragmatic) and B (Robust)...", total=None)
            plan_a, plan_b = await asyncio.gather(
                plan_service.generate_plan("planner_a", debate_goal, requirements_json, repo_map_json),
                plan_service.generate_plan("planner_b", debate_goal, requirements_json, repo_map_json),
            )
            orchestrator.save_run_artifact("plan_a.json", plan_a.model_dump())
            orchestrator.save_run_artifact("plan_b.json", plan_b.model_dump())
            await orchestrator.transition_to(ProjectPhase.PLANS_GENERATED)

            # Serialize each plan/critique once and reuse the strings across the
            # critique, rebuttal, and arbitration steps below.
            plan_a_json = plan_a.model_dump_json()
            plan_b_json = plan_b.model_dump_json()

            # 5. Cross-Critique (independent — run concurrently)
            log_step("plan/5: cross-critiquing Plans A and B", project_root=root, run_id=run_id, trace=True)
            progress.add_task(description="Critiquing Plans A and B...", total=None)
            critique_a, critique_b = await asyncio.gather(
                critique_service.generate_critique("critic_a", plan_b_json, requirements_json),
                critique_service.generate_critique("critic_b", plan_a_json, requirements_json),
            )
            orchestrator.save_run_artifact("critique_a.json", critique_a.model_dump())
            orchestrator.save_run_artifact("critique_b.json", critique_b.model_dump())
            await orchestrator.transition_to(ProjectPhase.CRITIQUES_GENERATED)
            critique_a_json = critique_a.model_dump_json()
            critique_b_json = critique_b.model_dump_json()

            # 6. Rebuttals (independent — run concurrently)
            log_step("plan/6: generating rebuttals", project_root=root, run_id=run_id, trace=True)
            progress.add_task(description="Generating rebuttals...", total=None)
            rebuttal_a, rebuttal_b = await asyncio.gather(
                critique_service.generate_rebuttal("planner_a", plan_a_json, critique_b_json),
                critique_service.generate_rebuttal("planner_b", plan_b_json, critique_a_json),
            )
            orchestrator.save_run_artifact("rebuttal_a.json", rebuttal_a.model_dump())
            orchestrator.save_run_artifact("rebuttal_b.json", rebuttal_b.model_dump())

            # 7. Arbitration
            log_step("plan/7: arbitrating final plan", project_root=root, run_id=run_id, trace=True)
            progress.add_task(description="Arbitrating final plan...", total=None)
            decision = await arbiter_service.arbitrate(
                debate_goal,
                requirements_json,
                plan_a_json,
                plan_b_json,
                critique_a_json,
                critique_b_json,
                rebuttal_a.model_dump_json(),
                rebuttal_b.model_dump_json()
            )
            orchestrator.save_run_artifact("decision.json", decision.model_dump())
            await orchestrator.transition_to(ProjectPhase.ARBITRATED)
            reconciled_findings = _reconcile_findings([*critique_a.findings, *critique_b.findings], decision)
            final_tasks = [task.model_copy(update={"status": "planned"}) for task in decision.final_tasks]

    console.print("[green]Planning complete![/green]")
    console.print(f"[blue]Prompt enhancement:[/blue] .devcouncil/runs/{run_id}/prompt_enhancement.json")
    if dry_run:
        console.print("[blue](DRY RUN: No actual LLM calls were made)[/blue]")
        if not persist:
            console.print("[blue](DRY RUN: Final requirements/tasks were not persisted)[/blue]")
    # Operationalize the spec's edge-case elaboration: attach every acceptance criterion
    # the planner left unlinked to a task that owns its requirement, so elaborated edges
    # (truncation semantics, error paths, boundaries) are actually built and per-criterion
    # verified instead of silently dropped — the gap that lets a gated run be no better
    # than the raw prompt.
    final_tasks, backfilled_acs = backfill_acceptance_criteria(final_tasks, decision.final_requirements)
    if backfilled_acs:
        console.print(
            f"[dim]Linked {len(backfilled_acs)} unmapped acceptance criterion(s) to owning task(s) "
            "so every elaborated behavior is verified.[/dim]"
        )
    # Ground planner-named files in the real repo before they become the scope
    # whitelist: repair a typo'd/renamed modify target to its true path so the
    # agent's correct write isn't reverted for pointing at a path the planner
    # misspelled. Only relaxes/repairs scope — never tightens it.
    final_tasks, planned_file_warnings = reconcile_planned_files(
        final_tasks, repo_files_from_map(repo_map)
    )
    for warning in planned_file_warnings:
        console.print(f"[yellow]{warning}[/yellow]")

    # The other half: add the real callers (repo-map dependents) of each writable
    # file so a caller the planner omitted isn't reverted mid-run. Only widens scope.
    final_tasks, scope_widen_warnings = expand_scope_with_dependents(
        final_tasks, repo_map.dependents, repo_files_from_map(repo_map)
    )
    for warning in scope_widen_warnings:
        console.print(f"[dim]{warning}[/dim]")

    final_tasks, difficulty_warnings = apply_plan_difficulty(final_tasks, decision.final_requirements)
    for warning in difficulty_warnings:
        console.print(f"[yellow]{warning}[/yellow]")

    console.print(f"Final Requirements: [bold]{len(decision.final_requirements)}[/bold]")
    console.print(f"Final Tasks: [bold]{len(final_tasks)}[/bold]")

    spec_output = _maybe_convert_blocking_questions(
        spec_output, config, console, orchestrator=orchestrator,
    )

    # 8. Check Gates
    log_step("plan/8: checking plan-approval gates", project_root=root, run_id=run_id, trace=True)
    policy = GatePolicy()
    result = policy.check_plan_approval(
        decision.final_requirements,
        final_tasks,
        assumptions=spec_output.assumptions,
        findings=reconciled_findings,
        blocking_questions=spec_output.blocking_questions,
    )
    if persist:
        with db.get_session() as session:
            GapRepository(session).delete_plan_gaps()

    if result.passed:
        if persist:
            with db.get_session() as session:
                PlanningStateRepository(session).replace_active_plan(
                    decision.final_requirements,
                    spec_output.assumptions,
                    final_tasks,
                    reconciled_findings,
                )
            # Pin THIS plan's enhancement so the executor reads the domain guidance tied to
            # the plan it runs (not a later run's by mtime).
            save_active_prompt_enhancement(root, prompt_enhancement)

        console.print("[green]Plan approved by gates.[/green]")
        logger.info("Plan approved by gates: %d task(s)", len(final_tasks))
        await orchestrator.transition_to(ProjectPhase.PLAN_APPROVED)
        log_step("plan/complete: approved", project_root=root, run_id=run_id, trace=True)
        return [task.id for task in final_tasks]
    else:
        if persist:
            with db.get_session() as session:
                gap_repo = GapRepository(session)
                for gap in result.gaps:
                    gap_repo.save(gap)
        console.print("[yellow]Plan generated but failed gates. See status for gaps.[/yellow]")
        logger.warning("Plan failed approval gates with %d gap(s)", len(result.gaps))
        await orchestrator.transition_to(ProjectPhase.AWAITING_USER_DECISIONS)
        log_step("plan/complete: awaiting user decisions", project_root=root, run_id=run_id, trace=True)
        return []

def _latest_run_with_decision(root: Path, run_id: str | None) -> Path | None:
    runs_dir = root / ".devcouncil" / "runs"
    if run_id:
        candidate = runs_dir / run_id
        return candidate if (candidate / "decision.json").exists() else None
    if not runs_dir.exists():
        return None
    candidates = [d for d in runs_dir.iterdir() if (d / "decision.json").exists()]
    if not candidates:
        return None
    return max(candidates, key=lambda d: (d / "decision.json").stat().st_mtime)


def approve(
    run_id: str | None = typer.Option(None, "--run-id", help="Run whose generated plan to approve (defaults to the most recent run with a decision)."),
    force: bool = typer.Option(False, "--force", help="Approve even if blocking gate gaps remain."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Approve a generated plan after reviewing gate gaps (AWAITING_USER_DECISIONS -> PLAN_APPROVED).
    """
    from devcouncil.planning.arbiter_service import ArbiterDecision
    from devcouncil.planning.critique_service import CritiqueOutput
    from devcouncil.planning.spec_service import SpecOutput

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev plan approve: run_id=%s force=%s", run_id or "latest", force)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    with log_stage("approve", project_root=root, run_id=run_id or "latest", force=force):
        log_step("approve/1: loading plan decision", project_root=root, trace=True)
        run_dir = _latest_run_with_decision(root, run_id)
        if run_dir is None:
            console.print("[red]No planning run with a decision was found. Run 'dev plan' first.[/red]")
            raise typer.Exit(code=1)

        decision = ArbiterDecision.model_validate_json((run_dir / "decision.json").read_text(encoding="utf-8"))
        spec_path = run_dir / "requirements.json"
        spec_output = (
            SpecOutput.model_validate_json(spec_path.read_text(encoding="utf-8")) if spec_path.exists() else None
        )

        config = load_config(root)
        if spec_output is not None:
            spec_output = _maybe_convert_blocking_questions(
                spec_output,
                config,
                console,
                artifact_path=spec_path if spec_path.exists() else None,
            )

        findings = []
        for name in ("critique_a.json", "critique_b.json"):
            critique_path = run_dir / name
            if critique_path.exists():
                findings.extend(CritiqueOutput.model_validate_json(critique_path.read_text(encoding="utf-8")).findings)
        reconciled_findings = _reconcile_findings(findings, decision)
        final_tasks = [task.model_copy(update={"status": "planned"}) for task in decision.final_tasks]
        final_tasks, _ = apply_plan_difficulty(final_tasks, decision.final_requirements)
        assumptions = spec_output.assumptions if spec_output else []

        policy = GatePolicy()
        log_step("approve/2: checking plan-approval gates", project_root=root)
        result = policy.check_plan_approval(
            decision.final_requirements,
            final_tasks,
            assumptions=assumptions,
            findings=reconciled_findings,
            blocking_questions=spec_output.blocking_questions if spec_output else [],
        )
        if not result.passed and not force:
            console.print("[yellow]Plan still fails approval gates:[/yellow]")
            for gap in result.gaps:
                marker = "[red][BLOCKING][/red] " if gap.blocking else ""
                console.print(f" - {marker}{gap.description} (Fix: {gap.recommended_fix})")
            console.print("Resolve the gaps and re-run 'dev plan', or use --force to approve anyway.")
            raise typer.Exit(code=1)

        with db.get_session() as session:
            GapRepository(session).delete_plan_gaps()
            PlanningStateRepository(session).replace_active_plan(
                decision.final_requirements,
                assumptions,
                final_tasks,
                reconciled_findings,
            )

        orchestrator = Orchestrator(root)
        try:
            asyncio.run(orchestrator.transition_to(ProjectPhase.PLAN_APPROVED))
        except ValueError as exc:
            console.print(f"[red]Cannot approve from the current project phase: {exc}[/red]")
            raise typer.Exit(code=1)
        console.print(f"[green]Plan from run {run_dir.name} approved ({len(final_tasks)} tasks).[/green]")
        console.print("Use 'dev tasks list' to see the planned tasks and 'dev run TASK-ID' to execute one.")
        log_step("approve/complete", project_root=root, run_id=run_dir.name, trace=True)


@app.command()
def plan(
    goal: str = typer.Argument(..., help="The goal of the implementation"),
    requirements_only: bool = typer.Option(False, "--requirements-only", help="Only generate requirements"),
    dry_run: bool = typer.Option(False, "--dry-run", help="Simulate planning without LLM calls"),
    quick: bool = typer.Option(
        False,
        "--quick",
        help="Rigor dial: skip the A/B debate, critique, rebuttal, and arbitration. "
        "One spec + one plan (~5 fewer model calls). Verification still gates every diff.",
    ),
    persist: bool = typer.Option(
        False,
        "--persist/--no-persist",
        help="Persist dry-run planning artifacts into the main state database.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Run the full planning cycle (Repo map -> Spec -> Plan A/B -> Critique -> Arbiter).
    """
    should_persist = persist or not dry_run
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev plan: quick=%s dry_run=%s requirements_only=%s", quick, dry_run, requirements_only)
    try:
        asyncio.run(run_plan_flow(goal, requirements_only, dry_run, should_persist, project_root, quick=quick))
    except (ProviderRequestError, StructuredOutputError) as exc:
        print_planning_error(exc)
        raise typer.Exit(code=1)


def print_planning_error(exc: Exception) -> None:
    """Render a planning/model failure as an actionable message instead of a traceback."""
    console.print(f"\n[red]Planning could not complete:[/red] {exc}")
    if isinstance(exc, StructuredOutputError):
        console.print(
            "[yellow]Tip:[/yellow] this role's model could not return valid structured JSON. "
            "Free/very small models often can't. Set a more capable model, e.g.\n"
            f"  [bold]dev config models --role {exc.role} --model anthropic/claude-sonnet-4.6[/bold]\n"
            "  (or set all roles: [bold]dev config models --model <model>[/bold])"
        )
    elif isinstance(exc, ProviderRequestError) and exc.status_code == 402:
        console.print(
            "[yellow]Tip:[/yellow] add credits at https://openrouter.ai/settings/credits, "
            "or switch to a free/cheaper model with [bold]dev config models --model <model>[/bold]."
        )
