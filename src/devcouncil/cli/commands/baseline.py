import json
import logging
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.storage.db import get_db
from devcouncil.verification.verifier import Verifier
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def baseline(
    force: bool = typer.Option(False, "--force", help="Overwrite an existing baseline snapshot."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Capture the current repo state as DevCouncil's verification baseline."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev baseline: force=%s", force)
    initialize_project(root, quiet=True)
    if not get_db(root):
        raise typer.Exit(code=1)

    with log_stage("baseline", project_root=root, force=force):
        log_step("baseline/1: capturing verification snapshot", project_root=root, trace=True)
        baseline_path = root / ".devcouncil" / "baseline.json"
        if baseline_path.exists() and not force:
            console.print("[yellow]Baseline already exists. Use --force to replace it.[/yellow]")
            raise typer.Exit(code=1)

        changed_files = Verifier(root).get_changed_files()
        payload = {
            "changed_files": changed_files,
            "note": "Files present in this snapshot are excluded from future task-scoped verification diffs.",
        }
        baseline_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
        console.print(f"[green]Captured baseline with {len(changed_files)} changed file(s).[/green]")
        log_step("baseline/complete", project_root=root, file_count=len(changed_files), trace=True)
