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
import asyncio
from pathlib import Path

app = typer.Typer()
console = Console()

async def run_repair_flow(project_root: Path = Path(".")):
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        return

    with db.get_session() as session:
        gap_repo = GapRepository(session)
        task_repo = TaskRepository(session)

        all_gaps = gap_repo.get_all()
        blocking_gaps = [g for g in all_gaps if g.blocking]
        
        if not blocking_gaps:
            console.print("[green]No blocking gaps found. Nothing to repair![/green]")
            return

        console.print(f"Found [bold]{len(blocking_gaps)}[/bold] blocking gaps. Orchestrating repair plan...")

        # Load router
        try:
            config = load_config(root)
            validate_model_provider(config.models.provider)
            api_key = get_api_key(config.models.provider, root)
        except (FileNotFoundError, ValueError) as e:
            console.print(f"[red]{e}[/red]")
            return
        
        provider = create_provider(config.models.provider, api_key, project_root=root)
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        router = ModelRouter(provider, role_config, project_root=root)
        repair_service = RepairService(router)
        context_builder = ContextBuilder(root)
        
        # Build minimal context for repair.
        project_context = context_builder.get_structure_summary()

        repair_output = await repair_service.generate_repair_plan(blocking_gaps, str(project_context))
        
        for task in repair_output.suggested_tasks:
            task.id = f"REPAIR-{task.id}"
            task_repo.save(task)
            console.print(f"  - Created intelligent repair task [bold]{task.id}[/bold]: {task.title}")

        console.print(f"\n[green]Successfully generated {len(repair_output.suggested_tasks)} repair tasks.[/green]")

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
