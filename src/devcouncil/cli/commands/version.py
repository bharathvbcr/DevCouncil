import logging
import typer
from rich.console import Console
import importlib.metadata

from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

@app.callback(invoke_without_command=True)
def version(ctx: typer.Context):
    """
    Display the current version of DevCouncil.
    """
    if ctx.invoked_subcommand is not None:
        return

    logger.info("dev version")
    with log_stage("version"):
        log_step("version/1: reading package metadata", trace=True)
        try:
            ver = importlib.metadata.version("devcouncil")
            console.print(f"DevCouncil version: [bold cyan]{ver}[/bold cyan]")
        except importlib.metadata.PackageNotFoundError:
            console.print("DevCouncil version: [yellow]unknown (editable/uninstalled)[/yellow]")
        log_step("version/complete", trace=True)
