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

AGENT_GUIDE_MARKER = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"


def _agent_guide_text(repo_map_path: Path, repo_root: Path) -> str:
    return "\n".join(
        [
            AGENT_GUIDE_MARKER,
            "",
            "# Agent Workspace Guide",
            "",
            "Use `.devcouncil/repo_map.json` as the primary file index for this workspace.",
            f"Repo map: `{repo_map_path.relative_to(repo_root).as_posix() if repo_map_path.is_relative_to(repo_root) else repo_map_path}`",
            "",
            "Workflow for agents:",
            "1. Open `.devcouncil/repo_map.json` before guessing at file locations.",
            "2. Use the `files` list to resolve module ownership and nearby siblings.",
            "3. Use `subsystems` for subsystem-level navigation (execution, verification, storage, etc.).",
            "4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.",
            "5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).",
            "6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.",
            "7. Run `dev map` again after large refactors to refresh the map.",
            "",
            "Important surfaces:",
            "1. `src/devcouncil/cli/main.py` for CLI composition.",
            "2. `src/devcouncil/app/orchestrator.py` and `src/devcouncil/app/state_machine.py` for lifecycle control.",
            "3. `src/devcouncil/artifacts/graph.py` and `src/devcouncil/storage/repositories.py` for persistence and evidence.",
            "4. `src/devcouncil/execution/` and `src/devcouncil/executors/` for task execution.",
            "5. `src/devcouncil/verification/` and `src/devcouncil/gating/` for verification and policy gates.",
            "6. `src/devcouncil/storage/` for persistence, SQL models, and repositories.",
            "",
            "If the map and source disagree, trust the source and regenerate the map.",
        ]
    )


def _write_agent_guides(repo_root: Path, repo_map_path: Path) -> None:
    for filename in ("AGENTS.md", "CLAUDE.md"):
        path = repo_root / filename
        if path.exists():
            existing = path.read_text(encoding="utf-8")
            if AGENT_GUIDE_MARKER not in existing:
                continue
        path.write_text(_agent_guide_text(repo_map_path, repo_root) + "\n", encoding="utf-8")


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
    _write_agent_guides(root, output)
    if graph_context.available:
        graph_output = output.with_name("code_review_graph_context.json")
        graph_output.write_text(graph_context.model_dump_json(indent=2), encoding="utf-8")
        status_console.print(f"[green]Wrote code-review-graph context to {graph_output}[/green]")
    typer.echo(json.dumps(repo_map.model_dump(), indent=2))
    status_console.print(f"[green]Wrote repository map to {output}[/green]")
