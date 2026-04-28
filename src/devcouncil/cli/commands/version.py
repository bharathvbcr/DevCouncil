import typer
from rich.console import Console
import importlib.metadata

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def version(ctx: typer.Context):
    """
    Display the current version of DevCouncil.
    """
    if ctx.invoked_subcommand is not None:
        return
        
    try:
        ver = importlib.metadata.version("devcouncil")
        console.print(f"DevCouncil version: [bold cyan]{ver}[/bold cyan]")
    except importlib.metadata.PackageNotFoundError:
        console.print("DevCouncil version: [yellow]unknown (editable/uninstalled)[/yellow]")
