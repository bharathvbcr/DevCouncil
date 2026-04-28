import typer
import subprocess
import os
import shutil
from rich.console import Console
from rich.table import Table

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def doctor(ctx: typer.Context):
    """
    Check the environment for DevCouncil prerequisites.
    """
    if ctx.invoked_subcommand is not None:
        return

    def _command_version(command: list[str]) -> str | None:
        try:
            return subprocess.check_output(
                command,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=10,
            ).splitlines()[0].strip()
        except Exception:
            return None

    table = Table(title="DevCouncil Doctor Check")
    table.add_column("Component", style="cyan")
    table.add_column("Status", style="magenta")
    table.add_column("Notes", style="green")

    # Check Git
    git_ver = _command_version(["git", "--version"])
    if git_ver:
        table.add_row("Git", "[green]OK[/green]", git_ver)
    else:
        table.add_row("Git", "[red]Missing[/red]", "Git is required for repo mapping and checkpoints.")

    # Check uv
    uv_ver = _command_version(["uv", "--version"])
    if uv_ver:
        table.add_row("uv", "[green]OK[/green]", uv_ver)
    else:
        table.add_row("uv", "[red]Missing[/red]", "Install uv to run or install DevCouncil.")

    # Check CLI shims
    if shutil.which("devcouncil"):
        table.add_row("devcouncil CLI", "[green]OK[/green]", "Found on PATH.")
    else:
        table.add_row("devcouncil CLI", "[yellow]Missing[/yellow]", "Run via 'uv run devcouncil' or install with 'uv tool install --force .'.")

    # Check ripgrep
    rg_ver = _command_version(["rg", "--version"])
    if rg_ver:
        table.add_row("ripgrep (rg)", "[green]OK[/green]", rg_ver)
    else:
        table.add_row("ripgrep (rg)", "[yellow]Missing[/yellow]", "ripgrep is highly recommended for fast repo mapping.")

    # Check OpenRouter API Key
    if os.environ.get("OPENROUTER_API_KEY"):
        table.add_row("OPENROUTER_API_KEY", "[green]OK[/green]", "Found in environment.")
    else:
        table.add_row("OPENROUTER_API_KEY", "[yellow]Missing[/yellow]", "Required if using OpenRouter provider.")

    console.print(table)
