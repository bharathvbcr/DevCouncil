import json
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.storage.db import get_db

console = Console()
status_console = Console(stderr=True)


def map_repo(
    goal: str = typer.Argument("", help="Goal text used for candidate-file ranking."),
    output: Path = typer.Option(
        Path(".devcouncil/repo_map.json"),
        "--output",
        "-o",
        help="Path to write repo_map.json.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Build the deterministic repository map without calling an LLM."""
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    if not get_db(root):
        raise typer.Exit(code=1)

    repo_map = RepoMapper(root).map_repo(goal)
    graph_context = CodeReviewGraphAdapter(root).get_context()
    output = output if output.is_absolute() else root / output
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(repo_map.model_dump_json(indent=2), encoding="utf-8")
    if graph_context.available:
        graph_output = output.with_name("code_review_graph_context.json")
        graph_output.write_text(graph_context.model_dump_json(indent=2), encoding="utf-8")
        status_console.print(f"[green]Wrote code-review-graph context to {graph_output}[/green]")
    typer.echo(json.dumps(repo_map.model_dump(), indent=2))
    status_console.print(f"[green]Wrote repository map to {output}[/green]")
