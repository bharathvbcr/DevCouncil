import typer
from rich.console import Console
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, GapRepository
from devcouncil.planning.repair_service import RepairService
from devcouncil.execution.context_builder import ContextBuilder
from devcouncil.llm.provider import create_provider, validate_model_provider
from devcouncil.llm.router import ModelRouter
from devcouncil.app.config import load_config, get_api_key
from devcouncil.cli.commands.init import initialize_project
from devcouncil.telemetry.stages import log_stage, log_step
import asyncio
import logging
from pathlib import Path

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

async def run_repair_flow(project_root: Path = Path(".")):
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        return

    with log_stage("repair", project_root=root):
        log_step("repair/1: loading blocking gaps", project_root=root)
        with db.get_session() as session:
            gap_repo = GapRepository(session)
            task_repo = TaskRepository(session)

            all_gaps = gap_repo.get_all()
            blocking_gaps = [g for g in all_gaps if g.blocking]

            if not blocking_gaps:
                logger.info("dev repair: no blocking gaps; nothing to repair")
                console.print("[green]No blocking gaps found. Nothing to repair![/green]")
                return

            logger.info("dev repair: %d blocking gap(s) across %d task(s)", len(blocking_gaps), len({g.task_id for g in blocking_gaps if g.task_id}))
            console.print(f"Found [bold]{len(blocking_gaps)}[/bold] blocking gaps. Orchestrating repair plan...")

            # Load router when credentials are available. Correction manifests have a
            # deterministic fallback path, so missing model credentials must not block
            # repair artifact generation.
            repair_service = None
            config = None
            try:
                config = load_config(root)
                validate_model_provider(config.models.provider)
                api_key = get_api_key(config.models.provider, root)
            except (FileNotFoundError, ValueError) as e:
                logger.warning("dev repair: no LLM provider (%s); using deterministic manifest fallback", e)
                console.print(f"[yellow]{e}[/yellow]")
            else:
                provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)
                role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
                router = ModelRouter(provider, role_config, project_root=root)
                repair_service = RepairService(router)
            context_builder = ContextBuilder(root)

            # Build minimal context for repair.
            project_context = context_builder.get_structure_summary()

            from devcouncil.planning.correction_manifest import write_correction_manifest

            task_ids = {gap.task_id for gap in blocking_gaps if gap.task_id}
            log_step("repair/2: writing correction manifests", project_root=root, task_count=len(task_ids))
            for scoped_task_id in task_ids:
                if scoped_task_id:
                    path = write_correction_manifest(root, scoped_task_id, repair_service=repair_service, config=config)
                    if path:
                        console.print(f"  - Wrote correction manifest [dim]{path}[/dim]")

            repair_count = 0
            if repair_service is not None:
                log_step("repair/3: generating LLM repair plan", project_root=root)
                repair_output = await repair_service.generate_repair_plan(blocking_gaps, str(project_context))
                for task in repair_output.suggested_tasks:
                    task.id = f"REPAIR-{task.id}"
                    task_repo.save(task)
                    repair_count += 1
                    console.print(f"  - Created intelligent repair task [bold]{task.id}[/bold]: {task.title}")

            log_step(
                "repair/complete",
                project_root=root,
                repair_tasks=repair_count,
                trace=True,
            )
            logger.info("dev repair complete: generated %d repair task(s)", repair_count)
            console.print(f"\n[green]Successfully generated {repair_count} repair tasks.[/green]")

@app.callback(invoke_without_command=True)
def repair(
    ctx: typer.Context,
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Convert blocking gaps into intelligent repair tasks using LLM inference.
    """
    if ctx.invoked_subcommand is not None:
        return

    asyncio.run(run_repair_flow(project_root))
