import typer
import subprocess
import os
import shutil
from pathlib import Path
from rich.console import Console
from rich.table import Table

app = typer.Typer()
console = Console()

def render_doctor_check():
    def _command_version(command: list[str]) -> str | None:
        executable = shutil.which(command[0])
        if not executable:
            return None

        resolved_command = [executable, *command[1:]]
        use_shell = os.name == "nt" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
        invocation = subprocess.list2cmdline(resolved_command) if use_shell else resolved_command
        try:
            return subprocess.check_output(
                invocation,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                shell=use_shell,
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

    # Check supported coding CLIs
    codex_ver = _command_version(["codex", "--version"])
    if codex_ver:
        table.add_row("Codex CLI", "[green]OK[/green]", f"{codex_ver}. Setup: dev integrate codex --apply")
    else:
        table.add_row("Codex CLI", "[yellow]Missing[/yellow]", "Optional. Install Codex, then run 'dev integrate codex --apply'.")

    gemini_ver = _command_version(["gemini", "--version"])
    if gemini_ver:
        table.add_row("Gemini CLI", "[green]OK[/green]", f"{gemini_ver}. Setup: dev integrate gemini --apply")
    else:
        table.add_row("Gemini CLI", "[yellow]Missing[/yellow]", "Optional. Install Gemini CLI, then run 'dev integrate gemini --apply'.")

    # Check OpenRouter API Key
    if os.environ.get("OPENROUTER_API_KEY"):
        table.add_row("OPENROUTER_API_KEY", "[green]OK[/green]", "Found in environment.")
    else:
        table.add_row("OPENROUTER_API_KEY", "[yellow]Missing[/yellow]", "Required if using OpenRouter provider.")

    console.print(table)


@app.callback(invoke_without_command=True)
def doctor(ctx: typer.Context):
    """
    Check the environment for DevCouncil prerequisites.
    """
    if ctx.invoked_subcommand is not None:
        return

    render_doctor_check()
