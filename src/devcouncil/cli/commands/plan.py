import typer
import asyncio
import json
import datetime
from rich.console import Console
from rich.panel import Panel
from rich.progress import Progress, SpinnerColumn, TextColumn
from pathlib import Path

from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import (
    RequirementRepository,
    AssumptionRepository,
    TaskRepository,
    CritiqueFindingRepository,
    GapRepository,
)
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.llm.provider import OpenRouterProvider, MockProvider
from devcouncil.llm.router import ModelRouter
from devcouncil.planning.spec_service import SpecService
from devcouncil.planning.plan_service import PlanService
from devcouncil.planning.critique_service import CritiqueService
from devcouncil.planning.arbiter_service import ArbiterService
from devcouncil.gating.policy import GatePolicy
from devcouncil.app.orchestrator import Orchestrator
from devcouncil.app.state_machine import ProjectPhase
from devcouncil.app.config import load_config, get_api_key

app = typer.Typer()
console = Console()

async def run_plan_flow(
    goal: str,
    requirements_only: bool = False,
    dry_run: bool = False,
    persist: bool = True,
):
    db = get_db()
    if not db:
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        return

    # Load validated config
    config = load_config(Path("."))

    api_key = None
    if not dry_run:
        try:
            api_key = get_api_key(config.models.provider)
        except ValueError as e:
            console.print(f"[red]{e}[/red]")
            return

    orchestrator = Orchestrator(Path("."), persist_state=persist)
    orchestrator.reset_state_machine(ProjectPhase.NEW)
    run_id = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-plan"
    await orchestrator.start_run(run_id, goal)

    if dry_run:
        # Override config models to be unique roles for mock mapping
        config.models.roles["spec_writer"].model = "mock/spec_writer"
        config.models.roles["planner_a"].model = "mock/planner_a"
        config.models.roles["planner_b"].model = "mock/planner_b"
        config.models.roles["critic_a"].model = "mock/critic_a"
        config.models.roles["critic_b"].model = "mock/critic_b"
        config.models.roles["arbiter"].model = "mock/arbiter"

        provider = MockProvider()
        provider.responses = {
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
        provider = OpenRouterProvider(api_key)

    # Build role config after dry-run overrides so mocks are routed correctly.
    role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
    router = ModelRouter(provider, role_config)
    
    spec_service = SpecService(router)
    plan_service = PlanService(router)
    critique_service = CritiqueService(router)
    arbiter_service = ArbiterService(router)
    mapper = RepoMapper(Path("."))

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
        graph_context = CodeReviewGraphAdapter(Path(".")).get_context()
        if graph_context.available:
            orchestrator.save_run_artifact("code_review_graph_context.json", graph_context.model_dump())
        await orchestrator.transition_to(ProjectPhase.REPO_MAPPED)

        # 2. Spec / Requirements
        progress.add_task(description="Generating requirements...", total=None)
        spec_output = await spec_service.generate_spec(goal, repo_map_json)
        orchestrator.save_run_artifact("requirements.json", spec_output.model_dump())
        await orchestrator.transition_to(ProjectPhase.REQUIREMENTS_DRAFTED)
        
        if requirements_only:
            console.print(Panel(f"Found {len(spec_output.requirements)} requirements.", title="Requirements Generated"))
            return

        # 3. Independent Plans
        progress.add_task(description="Generating Plan A (Pragmatic)...", total=None)
        plan_a = await plan_service.generate_plan("planner_a", goal, json.dumps([r.model_dump() for r in spec_output.requirements]), repo_map_json)
        orchestrator.save_run_artifact("plan_a.json", plan_a.model_dump())
        
        progress.add_task(description="Generating Plan B (Robust)...", total=None)
        plan_b = await plan_service.generate_plan("planner_b", goal, json.dumps([r.model_dump() for r in spec_output.requirements]), repo_map_json)
        orchestrator.save_run_artifact("plan_b.json", plan_b.model_dump())
        await orchestrator.transition_to(ProjectPhase.PLANS_GENERATED)

        # 4. Cross-Critique
        progress.add_task(description="Critiquing Plan B...", total=None)
        critique_a = await critique_service.generate_critique("critic_a", plan_b.model_dump_json(), json.dumps([r.model_dump() for r in spec_output.requirements]))
        orchestrator.save_run_artifact("critique_a.json", critique_a.model_dump())
        
        progress.add_task(description="Critiquing Plan A...", total=None)
        critique_b = await critique_service.generate_critique("critic_b", plan_a.model_dump_json(), json.dumps([r.model_dump() for r in spec_output.requirements]))
        orchestrator.save_run_artifact("critique_b.json", critique_b.model_dump())
        await orchestrator.transition_to(ProjectPhase.CRITIQUES_GENERATED)

        # 5. Rebuttals
        progress.add_task(description="Generating rebuttals...", total=None)
        rebuttal_a = await critique_service.generate_rebuttal("planner_a", plan_a.model_dump_json(), critique_b.model_dump_json())
        orchestrator.save_run_artifact("rebuttal_a.json", rebuttal_a.model_dump())
        rebuttal_b = await critique_service.generate_rebuttal("planner_b", plan_b.model_dump_json(), critique_a.model_dump_json())
        orchestrator.save_run_artifact("rebuttal_b.json", rebuttal_b.model_dump())

        # 6. Arbitration
        progress.add_task(description="Arbitrating final plan...", total=None)
        decision = await arbiter_service.arbitrate(
            goal,
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

        # 7. Save to DB unless this is a non-persistent dry run.
        if persist:
            with db.get_session() as session:
                req_repo = RequirementRepository(session)
                assumption_repo = AssumptionRepository(session)
                task_repo = TaskRepository(session)
                finding_repo = CritiqueFindingRepository(session)
                
                for req in decision.final_requirements:
                    req_repo.save(req)

                for assumption in spec_output.assumptions:
                    assumption_repo.save(assumption)
                
                for task in decision.final_tasks:
                    task_repo.save(task)

                for finding in [*critique_a.findings, *critique_b.findings]:
                    finding_repo.save(finding)

    console.print("[green]Planning complete![/green]")
    if dry_run:
        console.print("[blue](DRY RUN: No actual LLM calls were made)[/blue]")
        if not persist:
            console.print("[blue](DRY RUN: Final requirements/tasks were not persisted)[/blue]")
    console.print(f"Final Requirements: [bold]{len(decision.final_requirements)}[/bold]")
    console.print(f"Final Tasks: [bold]{len(decision.final_tasks)}[/bold]")
    
    # 8. Check Gates
    policy = GatePolicy()
    result = policy.check_plan_approval(
        decision.final_requirements,
        decision.final_tasks,
        assumptions=spec_output.assumptions,
        findings=[*critique_a.findings, *critique_b.findings],
        blocking_questions=spec_output.blocking_questions,
    )
    if persist:
        with db.get_session() as session:
            GapRepository(session).delete_plan_gaps()

    if result.passed:
        console.print("[green]Plan approved by gates.[/green]")
        await orchestrator.transition_to(ProjectPhase.PLAN_APPROVED)
    else:
        if persist:
            with db.get_session() as session:
                gap_repo = GapRepository(session)
                for gap in result.gaps:
                    gap_repo.save(gap)
        console.print("[yellow]Plan generated but failed gates. See status for gaps.[/yellow]")
        await orchestrator.transition_to(ProjectPhase.AWAITING_USER_DECISIONS)

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
):
    """
    Run the full planning cycle (Repo map -> Spec -> Plan A/B -> Critique -> Arbiter).
    """
    should_persist = persist or not dry_run
    asyncio.run(run_plan_flow(goal, requirements_only, dry_run, should_persist))
