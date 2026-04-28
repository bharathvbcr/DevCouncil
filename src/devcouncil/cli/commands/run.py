import typer
import subprocess
import json
from rich.console import Console
from pathlib import Path
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, RequirementRepository
from devcouncil.executors.mini_swe import MiniSWEExecutor
from devcouncil.executors.openhands import OpenHandsExecutor
from devcouncil.executors.native.agent import NativeAgent
from devcouncil.llm.provider import OpenRouterProvider
from devcouncil.llm.router import ModelRouter
from devcouncil.app.config import load_config, get_api_key
from devcouncil.domain.evidence import CommandResult, DiffEvidence, TestEvidence
from devcouncil.storage.repositories import GapRepository, EvidenceRepository, StateRepository
from devcouncil.verification.verifier import Verifier
from devcouncil.app.state_machine import ProjectPhase

console = Console()

def _current_changed_files() -> list[str]:
    from devcouncil.verification.verifier import Verifier

    return Verifier(Path(".")).get_changed_files()

def _capture_after_patch(task_id: str):
    """Capture the diff after task execution for use by rollback."""
    try:
        checkpoint_dir = Path(".devcouncil/checkpoints")
        checkpoint_dir.mkdir(exist_ok=True)
        diff = subprocess.check_output(["git", "diff", "HEAD"]).decode("utf-8", errors="ignore")
        if diff:
            with open(checkpoint_dir / f"{task_id}-after.patch", "w", encoding="utf-8") as f:
                f.write(diff)
    except Exception:
        pass  # Non-critical — don't block execution

def _capture_before_snapshot(task_id: str):
    checkpoint_dir = Path(".devcouncil/checkpoints")
    checkpoint_dir.mkdir(exist_ok=True)
    snapshot = {
        "task_id": task_id,
        "changed_files": _current_changed_files(),
    }
    (checkpoint_dir / f"{task_id}-before.json").write_text(
        json.dumps(snapshot, indent=2),
        encoding="utf-8",
    )

def _record_project_phase(session, phase: ProjectPhase):
    StateRepository(session).record_phase(phase.value)

def _verify_after_execution(session, task, reqs, router=None) -> bool:
    """Run deterministic verification after an automated executor finishes."""
    import asyncio

    verifier = Verifier(Path("."), router=router)
    gaps, evidence = asyncio.run(verifier.verify_task(task, reqs))

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

def run(
    task_id: str = typer.Argument(..., help="ID of the task to run"),
    executor: str = typer.Option("manual", "--executor", "-e", help="Executor to use (manual, shell)"),
):
    """
    Execute a specific task.
    """
    db = get_db()
    if not db:
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        return

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        task = task_repo.get_by_id(task_id)
        if not task:
            console.print(f"[red]Task {task_id} not found.[/red]")
            return

        from devcouncil.gating.policy import GatePolicy
        policy = GatePolicy()
        gate_result = policy.check_task_ready(task, Path("."))
        if not gate_result.passed:
            console.print(f"[red]Task {task_id} is not ready for execution.[/red]")
            for gap in gate_result.gaps:
                if gap.blocking:
                    console.print(f" - [red][BLOCKING][/red] {gap.description} (Fix: {gap.recommended_fix})")
            return

        console.print(f"Running task [bold]{task_id}[/bold] using [bold]{executor}[/bold] executor...")

        # 1. Create Git checkpoint
        try:
            checkpoint_dir = Path(".devcouncil/checkpoints")
            checkpoint_dir.mkdir(exist_ok=True)
            _capture_before_snapshot(task_id)
            diff = subprocess.check_output(["git", "diff", "HEAD"]).decode("utf-8", errors="ignore")
            if diff:
                with open(checkpoint_dir / f"{task_id}-before.patch", "w", encoding="utf-8") as f:
                    f.write(diff)
                console.print(f"Created git checkpoint at {checkpoint_dir}/{task_id}-before.patch")
        except Exception as e:
            console.print(f"[yellow]Warning: Failed to create git checkpoint: {e}[/yellow]")

        if executor == "manual":
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            task.status = "running"
            task_repo.save(task)
            console.print(f"\n[green]Task {task_id} is now marked as RUNNING.[/green]")
            console.print("Use 'dev prompt TASK-ID' to get the prompt for this task.")
            console.print("When finished, use 'dev verify TASK-ID' to check the results.")
        elif executor == "mini":
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            mini_executor = MiniSWEExecutor(Path("."))
            exec_result = mini_executor.run_task(task, reqs)
            _capture_after_patch(task_id)
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(session, task, reqs)
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                if verified:
                    console.print(f"\n[green]mini-SWE-agent finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]mini-SWE-agent finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                console.print("\n[red]mini-SWE-agent failed to start or execute.[/red]")
        elif executor == "openhands":
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            oh_executor = OpenHandsExecutor(Path("."))
            exec_result = oh_executor.run_task(task, reqs)
            _capture_after_patch(task_id)
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(session, task, reqs)
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                if verified:
                    console.print(f"\n[green]OpenHands finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]OpenHands finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                console.print("\n[red]OpenHands failed to start or execute.[/red]")
        elif executor == "native":
            # Load config for model routing and permissions
            try:
                config = load_config(Path("."))
                api_key = get_api_key(config.models.provider)
            except (FileNotFoundError, ValueError) as e:
                console.print(f"[red]{e}[/red]")
                return
            
            provider = OpenRouterProvider(api_key)
            role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
            router = ModelRouter(provider, role_config)
            
            # Setup Permission System
            from devcouncil.execution.permissions import PermissionPolicy, PermissionManager
            from devcouncil.execution.task_runner import TaskRunner
            
            # Populate policy from config commands
            allowed_cmds = config.commands.test + config.commands.lint + config.commands.typecheck
            policy = PermissionPolicy(
                allowed_shell_commands=allowed_cmds,
            )
            perm_manager = PermissionManager(policy, Path("."))
            task_runner = TaskRunner(Path("."), perm_manager)
            
            req_repo = RequirementRepository(session)
            reqs = req_repo.get_all()
            
            import asyncio
            _record_project_phase(session, ProjectPhase.TASK_EXECUTING)
            agent = NativeAgent(router, task_runner)
            exec_result = asyncio.run(agent.run_task(task, reqs))
            _capture_after_patch(task_id)
            
            if exec_result.success:
                _record_project_phase(session, ProjectPhase.TASK_VERIFYING)
                verified = _verify_after_execution(session, task, reqs, router=router)
                _record_project_phase(
                    session,
                    ProjectPhase.TASK_VERIFIED if verified else ProjectPhase.TASK_BLOCKED,
                )
                task_repo.save(task)
                if verified:
                    console.print(f"\n[green]Native agent finished and task {task_id} verified.[/green]")
                else:
                    console.print(f"\n[yellow]Native agent finished, but task {task_id} is blocked by verification gaps.[/yellow]")
            else:
                console.print("\n[red]Native agent failed during execution.[/red]")
        else:
            console.print(f"[red]Executor {executor} not yet implemented.[/red]")
