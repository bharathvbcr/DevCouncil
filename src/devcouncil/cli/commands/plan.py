import typer
import asyncio
import json
import datetime
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
from devcouncil.llm.provider import Provider, MockProvider, build_role_model_config, create_provider, validate_model_provider
from devcouncil.llm.router import ModelRouter
from devcouncil.planning.spec_service import SpecService
from devcouncil.planning.prompt_enhancer_service import PromptEnhancerService
from devcouncil.planning.plan_service import PlanService
from devcouncil.planning.critique_service import CritiqueService
from devcouncil.planning.arbiter_service import ArbiterService
from devcouncil.gating.policy import GatePolicy
from devcouncil.app.orchestrator import Orchestrator
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.app.config import ModelRoleConfig, load_config, get_api_key
from devcouncil.cli.commands.init import initialize_project
from devcouncil.telemetry.traces import TraceLogger

app = typer.Typer()
console = Console()

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

async def run_plan_flow(
    goal: str,
    requirements_only: bool = False,
    dry_run: bool = False,
    persist: bool = True,
    project_root: Path = Path("."),
):
    root = project_root.expanduser().resolve()
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
            "mock/prompt_enhancer": json.dumps({
                "original_goal": goal,
                "enhanced_goal": (
                    f"Plan and implement {goal} using the mapped repository's "
                    "existing patterns, tests, and verification gates."
                ),
                "codebase_context": ["Use the repository map to target existing application and test structure."],
                "debate_focus": ["Compare minimal implementation scope against production-readiness concerns."],
                "constraints": [f"Do not broaden beyond: {goal}."]
            }),
            "mock/spec_writer": json.dumps({
                "requirements": [{"id": "REQ-001", "title": "Mock Req", "description": "Desc", "priority": "high", "source": "user", "acceptance_criteria": []}],
                "assumptions": [],
                "blocking_questions": []
            }),
            "mock/planner_a": [
                json.dumps({
                    "id": "PLAN-A", "rationale": "Simple", "tasks": [{"id": "TASK-001", "title": "Mock Task", "description": "Desc", "requirement_ids": ["REQ-001"], "acceptance_criterion_ids": [], "planned_files": [], "expected_tests": [], "allowed_commands": [], "status": "planned"}]
                }),
                json.dumps({"rebuttals": []})
            ],
            "mock/planner_b": [
                json.dumps({
                    "id": "PLAN-B", "rationale": "Robust", "tasks": [{"id": "TASK-001", "title": "Mock Task", "description": "Desc", "requirement_ids": ["REQ-001"], "acceptance_criterion_ids": [], "planned_files": [], "expected_tests": [], "allowed_commands": [], "status": "planned"}]
                }),
                json.dumps({"rebuttals": []})
            ],
            "mock/critic_a": '{"findings": []}',
            "mock/critic_b": '{"findings": []}',
            "mock/arbiter": json.dumps({
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
        provider = create_provider(config.models.provider, api_key, project_root=root)

    # Build role config after dry-run overrides so mocks are routed correctly.
    role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
    router = ModelRouter(provider, role_config, project_root=root)
    
    prompt_enhancer = PromptEnhancerService(router)
    spec_service = SpecService(router)
    plan_service = PlanService(router)
    critique_service = CritiqueService(router)
    arbiter_service = ArbiterService(router)
    mapper = RepoMapper(root)

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        transient=True,
    ) as progress:
        # 1. Repo Map
        progress.add_task(description="Mapping repository...", total=None)
        repo_map = mapper.map_repo(goal)
        repo_map_json = repo_map.model_dump_json(indent=2)
        orchestrator.save_run_artifact("repo_map.json", json.loads(repo_map_json))
        graph_context = CodeReviewGraphAdapter(root).get_context()
        if graph_context.available:
            orchestrator.save_run_artifact("code_review_graph_context.json", graph_context.model_dump())
        await orchestrator.transition_to(ProjectPhase.REPO_MAPPED)

        # 2. Codebase-specific prompt enhancement
        progress.add_task(description="Enhancing prompt for codebase debate...", total=None)
        prompt_enhancement = await prompt_enhancer.enhance_prompt(
            goal,
            repo_map_json,
            graph_context.model_dump_json() if graph_context.available else None,
        )
        debate_goal = prompt_enhancement.debate_prompt()
        orchestrator.save_run_artifact("prompt_enhancement.json", prompt_enhancement.model_dump())
        TraceLogger(root).log_event(
            "prompt_enhanced",
            {
                "original_goal": goal,
                "enhanced_goal": prompt_enhancement.enhanced_goal,
                "codebase_context_count": len(prompt_enhancement.codebase_context),
                "constraint_count": len(prompt_enhancement.constraints),
                "debate_focus_count": len(prompt_enhancement.debate_focus),
                "artifact": f".devcouncil/runs/{run_id}/prompt_enhancement.json",
            },
            run_id=run_id,
            summary="Prompt enhanced for codebase-specific debate.",
        )

        # 3. Spec / Requirements
        progress.add_task(description="Generating requirements...", total=None)
        spec_output = await spec_service.generate_spec(debate_goal, repo_map_json)
        orchestrator.save_run_artifact("requirements.json", spec_output.model_dump())
        await orchestrator.transition_to(ProjectPhase.REQUIREMENTS_DRAFTED)
        
        if requirements_only:
            console.print(Panel(f"Found {len(spec_output.requirements)} requirements.", title="Requirements Generated"))
            return []

        # 4. Independent Plans
        progress.add_task(description="Generating Plan A (Pragmatic)...", total=None)
        plan_a = await plan_service.generate_plan("planner_a", debate_goal, json.dumps([r.model_dump() for r in spec_output.requirements]), repo_map_json)
        orchestrator.save_run_artifact("plan_a.json", plan_a.model_dump())
        
        progress.add_task(description="Generating Plan B (Robust)...", total=None)
        plan_b = await plan_service.generate_plan("planner_b", debate_goal, json.dumps([r.model_dump() for r in spec_output.requirements]), repo_map_json)
        orchestrator.save_run_artifact("plan_b.json", plan_b.model_dump())
        await orchestrator.transition_to(ProjectPhase.PLANS_GENERATED)

        # 5. Cross-Critique
        progress.add_task(description="Critiquing Plan B...", total=None)
        critique_a = await critique_service.generate_critique("critic_a", plan_b.model_dump_json(), json.dumps([r.model_dump() for r in spec_output.requirements]))
        orchestrator.save_run_artifact("critique_a.json", critique_a.model_dump())
        
        progress.add_task(description="Critiquing Plan A...", total=None)
        critique_b = await critique_service.generate_critique("critic_b", plan_a.model_dump_json(), json.dumps([r.model_dump() for r in spec_output.requirements]))
        orchestrator.save_run_artifact("critique_b.json", critique_b.model_dump())
        await orchestrator.transition_to(ProjectPhase.CRITIQUES_GENERATED)

        # 6. Rebuttals
        progress.add_task(description="Generating rebuttals...", total=None)
        rebuttal_a = await critique_service.generate_rebuttal("planner_a", plan_a.model_dump_json(), critique_b.model_dump_json())
        orchestrator.save_run_artifact("rebuttal_a.json", rebuttal_a.model_dump())
        rebuttal_b = await critique_service.generate_rebuttal("planner_b", plan_b.model_dump_json(), critique_a.model_dump_json())
        orchestrator.save_run_artifact("rebuttal_b.json", rebuttal_b.model_dump())

        # 7. Arbitration
        progress.add_task(description="Arbitrating final plan...", total=None)
        decision = await arbiter_service.arbitrate(
            debate_goal,
            json.dumps([r.model_dump() for r in spec_output.requirements]),
            plan_a.model_dump_json(),
            plan_b.model_dump_json(),
            critique_a.model_dump_json(),
            critique_b.model_dump_json(),
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
    console.print(f"Final Requirements: [bold]{len(decision.final_requirements)}[/bold]")
    console.print(f"Final Tasks: [bold]{len(final_tasks)}[/bold]")
    
    # 8. Check Gates
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

        console.print("[green]Plan approved by gates.[/green]")
        await orchestrator.transition_to(ProjectPhase.PLAN_APPROVED)
        return [task.id for task in final_tasks]
    else:
        if persist:
            with db.get_session() as session:
                gap_repo = GapRepository(session)
                for gap in result.gaps:
                    gap_repo.save(gap)
        console.print("[yellow]Plan generated but failed gates. See status for gaps.[/yellow]")
        await orchestrator.transition_to(ProjectPhase.AWAITING_USER_DECISIONS)
        return []

@app.command()
def plan(
    goal: str = typer.Argument(..., help="The goal of the implementation"),
    requirements_only: bool = typer.Option(False, "--requirements-only", help="Only generate requirements"),
    dry_run: bool = typer.Option(False, "--dry-run", help="Simulate planning without LLM calls"),
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
    asyncio.run(run_plan_flow(goal, requirements_only, dry_run, should_persist, project_root))
