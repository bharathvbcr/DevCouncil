import logging
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.repo.ci_scaffold import WORKFLOW_RELPATH, detect_stacks, scaffold_ci
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def scaffold_ci_command(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root to scaffold CI into."),
    force: bool = typer.Option(False, "--force", help="Overwrite an existing devcouncil.yml workflow."),
):
    """Write a starter GitHub Actions workflow derived from the configured commands."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev scaffold-ci: force=%s", force)
    if not (root / ".devcouncil").exists():
        console.print("[red]DevCouncil is not initialized here. Run 'dev setup' first.[/red]")
        raise typer.Exit(code=1)

    with log_stage("scaffold", project_root=root, force=force):
        log_step("scaffold/1: writing CI workflow", project_root=root, trace=True)
        target = scaffold_ci(root, force=force)
        if target is None:
            console.print(
                f"[yellow]{WORKFLOW_RELPATH.as_posix()} already exists. "
                f"Re-run with --force to overwrite.[/yellow]"
            )
            log_step("scaffold/complete", project_root=root, skipped=True, trace=True)
            return

        stacks = detect_stacks(root)
        detected = ", ".join(sorted(stacks)) if stacks else "none auto-detected"
        console.print(f"[green]Wrote {target.relative_to(root).as_posix()} (stacks: {detected}).[/green]")
        console.print("[dim]Review the setup/install steps and commands before relying on it.[/dim]")
        log_step("scaffold/complete", project_root=root, trace=True)
