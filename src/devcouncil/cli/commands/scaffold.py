from pathlib import Path

import typer
from rich.console import Console

from devcouncil.repo.ci_scaffold import WORKFLOW_RELPATH, detect_stacks, scaffold_ci

console = Console()


def scaffold_ci_command(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root to scaffold CI into."),
    force: bool = typer.Option(False, "--force", help="Overwrite an existing devcouncil.yml workflow."),
):
    """Write a starter GitHub Actions workflow derived from the configured commands."""
    root = project_root.expanduser().resolve()
    if not (root / ".devcouncil").exists():
        console.print("[red]DevCouncil is not initialized here. Run 'dev setup' first.[/red]")
        raise typer.Exit(code=1)

    target = scaffold_ci(root, force=force)
    if target is None:
        console.print(
            f"[yellow]{WORKFLOW_RELPATH.as_posix()} already exists. "
            f"Re-run with --force to overwrite.[/yellow]"
        )
        return

    stacks = detect_stacks(root)
    detected = ", ".join(sorted(stacks)) if stacks else "none auto-detected"
    console.print(f"[green]Wrote {target.relative_to(root).as_posix()} (stacks: {detected}).[/green]")
    console.print("[dim]Review the setup/install steps and commands before relying on it.[/dim]")
