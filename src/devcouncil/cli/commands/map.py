import json
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.storage.db import get_db

console = Console()


def map_repo(
    goal: str = typer.Argument("", help="Goal text used for candidate-file ranking."),
    output: Path = typer.Option(
        Path(".devcouncil/repo_map.json"),
        "--output",
        "-o",
        help="Path to write repo_map.json.",
    ),
):
    """Build the deterministic repository map without calling an LLM."""
    if not get_db():
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        raise typer.Exit(code=1)

    repo_map = RepoMapper(Path(".")).map_repo(goal)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(repo_map.model_dump_json(indent=2), encoding="utf-8")
    console.print(json.dumps(repo_map.model_dump(), indent=2))
    console.print(f"[green]Wrote repository map to {output}[/green]")
