import json
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.storage.db import get_db
from devcouncil.verification.verifier import Verifier

console = Console()


def baseline(
    force: bool = typer.Option(False, "--force", help="Overwrite an existing baseline snapshot."),
):
    """Capture the current repo state as DevCouncil's verification baseline."""
    if not get_db():
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        raise typer.Exit(code=1)

    baseline_path = Path(".devcouncil") / "baseline.json"
    if baseline_path.exists() and not force:
        console.print("[yellow]Baseline already exists. Use --force to replace it.[/yellow]")
        raise typer.Exit(code=1)

    changed_files = Verifier(Path(".")).get_changed_files()
    payload = {
        "changed_files": changed_files,
        "note": "Files present in this snapshot are excluded from future task-scoped verification diffs.",
    }
    baseline_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    console.print(f"[green]Captured baseline with {len(changed_files)} changed file(s).[/green]")
