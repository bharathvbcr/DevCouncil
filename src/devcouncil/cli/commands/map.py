import logging
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.storage.db import get_db
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json, write_model_json

console = Console()
status_console = Console(stderr=True)
logger = logging.getLogger(__name__)

AGENT_GUIDE_MARKER = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"


def _important_surfaces(repo_map: RepoMap) -> list[str]:
    """Derive the 'important surfaces' list from the computed map, so the guide points
    at THIS repo's real subsystems instead of hardcoded DevCouncil paths."""
    lines: list[str] = []
    for index, subsystem in enumerate(repo_map.subsystems[:6], start=1):
        lines.append(f"{index}. `{subsystem.area}/` — {subsystem.summary}")
    if not lines:
        for index, path in enumerate(repo_map.important_files[:6], start=1):
            lines.append(f"{index}. `{path}`")
    return lines or ["1. See `.devcouncil/repo_map.json` for the file index."]


def _wiki_index_rel(repo_root: Path) -> str | None:
    """Relative path to the codebase wiki index, when a generated wiki exists."""
    from devcouncil.cli.commands.wiki import wiki_dir_for

    index = wiki_dir_for(repo_root) / "index.md"
    if not index.is_file():
        return None
    try:
        return index.relative_to(repo_root).as_posix()
    except ValueError:
        return str(index)


def _agent_guide_text(repo_map_path: Path, repo_root: Path, repo_map: RepoMap) -> str:
    wiki_index = _wiki_index_rel(repo_root)
    wiki_lines = (
        [
            "",
            f"Codebase wiki: `{wiki_index}` — agent-facing subsystem docs (OKF bundle). "
            "Read the relevant subsystem page before working in it; refresh with `dev wiki update`.",
        ]
        if wiki_index
        else []
    )
    return "\n".join(
        [
            AGENT_GUIDE_MARKER,
            "",
            "# Agent Workspace Guide",
            "",
            "Use `.devcouncil/repo_map.json` as the primary file index for this workspace.",
            f"Repo map: `{repo_map_path.relative_to(repo_root).as_posix() if repo_map_path.is_relative_to(repo_root) else repo_map_path}`",
            *wiki_lines,
            "",
            "Workflow for agents:",
            "1. Open `.devcouncil/repo_map.json` before guessing at file locations.",
            "2. Use the `files` list to resolve module ownership and nearby siblings.",
            "3. Use `subsystems` for subsystem-level navigation.",
            "4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.",
            "5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).",
            "6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.",
            "7. Run `dev map` again after large refactors to refresh the map.",
            "",
            "Important surfaces:",
            *_important_surfaces(repo_map),
            "",
            "If the map and source disagree, trust the source and regenerate the map.",
        ]
    )


def _write_agent_guides(repo_root: Path, repo_map_path: Path, repo_map: RepoMap) -> None:
    for filename in ("AGENTS.md", "CLAUDE.md"):
        path = repo_root / filename
        if path.exists():
            existing = path.read_text(encoding="utf-8")
            if AGENT_GUIDE_MARKER not in existing:
                continue
        path.write_text(_agent_guide_text(repo_map_path, repo_root, repo_map) + "\n", encoding="utf-8")


def generate_map_artifacts(root: Path, output: Path, goal: str = "", *, scan_dependencies: bool = False) -> RepoMap:
    """Build the repo map and write repo_map.json + agent guides (no LLM, no re-init).

    Assumes ``.devcouncil/`` already exists. Shared by the ``dev map`` command and
    by project initialization so a freshly set-up repo is immediately navigable.
    ``scan_dependencies`` is opt-in (off for init and default mapping) because it can
    shell out to dependency auditors.
    """
    repo_map = RepoMapper(root).map_repo(goal, scan_dependencies=scan_dependencies)
    graph_context = CodeReviewGraphAdapter(root).get_context()
    output = output if output.is_absolute() else root / output
    output.parent.mkdir(parents=True, exist_ok=True)
    write_model_json(output, repo_map)
    _write_agent_guides(root, output, repo_map)
    if graph_context.available:
        graph_output = output.with_name("code_review_graph_context.json")
        write_model_json(graph_output, graph_context)
        status_console.print(f"[green]Wrote code-review-graph context to {graph_output}[/green]")
    return repo_map


def map_repo(
    goal: str = typer.Argument("", help="Goal text used for candidate-file ranking."),
    output: Path = typer.Option(
        Path(".devcouncil/repo_map.json"),
        "--output",
        "-o",
        help="Path to write repo_map.json.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    scan_deps: bool = typer.Option(
        False,
        "--scan-deps",
        help="Run available dependency auditors (pip-audit/npm audit/osv-scanner) and record dependency_risks in the map. Off by default.",
    ),
    refresh_wiki: bool = typer.Option(
        True,
        "--wiki/--no-wiki",
        help="After mapping, refresh stale codebase-wiki page skeletons when a wiki exists (no LLM calls).",
    ),
):
    """Build the deterministic repository map without calling an LLM."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev map: goal=%r scan_deps=%s", goal, scan_deps)
    initialize_project(root, quiet=True, with_map=False)
    if not get_db(root):
        raise typer.Exit(code=1)

    with log_stage("map", project_root=root, scan_deps=scan_deps):
        log_step("map/1: generating repository map", project_root=root, trace=True)
        output = output if output.is_absolute() else root / output
        repo_map = generate_map_artifacts(root, output, goal, scan_dependencies=scan_deps)
        typer.echo(dump_json(repo_map.model_dump(), indent=2))
        status_console.print(f"[green]Wrote repository map to {output}[/green]")
        if refresh_wiki:
            _refresh_wiki_skeletons(root, repo_map)
        log_step("map/complete", project_root=root, trace=True)


def graph_context_cmd(
    files: list[str] = typer.Option([], "--file", help="Changed files to scope blast-radius (repeatable)."),
    json_output: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Return code-review-graph context for the given files."""
    root = project_root.expanduser().resolve()
    context = CodeReviewGraphAdapter(root).get_context(files)
    if json_output:
        typer.echo(context.model_dump_json(indent=2))
        return
    if not context.available:
        console.print("[dim]Code-review graph integration is not available.[/dim]")
        return
    if context.impacted_files:
        console.print("[cyan]Impacted files:[/cyan] " + ", ".join(context.impacted_files[:20]))
    if context.related_tests:
        console.print("[cyan]Related tests:[/cyan] " + ", ".join(context.related_tests[:20]))


def _refresh_wiki_skeletons(root: Path, repo_map: RepoMap) -> None:
    """Keep the codebase wiki in step with the map — seamlessly, but deterministically.

    Only runs when a wiki was already generated (`dev wiki update` opted the repo in),
    and only rewrites pages whose repo-map slice changed. No model calls here: `dev map`
    must stay fast and offline; run `dev wiki update` for LLM-enriched prose on the
    refreshed pages.
    """
    try:
        from devcouncil.cli.commands.wiki import _project_name, wiki_dir_for
        from devcouncil.knowledge.wiki import generate_wiki, wiki_stale_pages

        wiki_dir = wiki_dir_for(root)
        if not (wiki_dir / "index.md").is_file():
            return
        stale = wiki_stale_pages(root, repo_map, wiki_dir)
        if not stale:
            return
        result = generate_wiki(root, repo_map, wiki_dir, project_name=_project_name(root))
        status_console.print(
            f"[green]Refreshed {len(result.changed)} stale wiki page(s)[/green] "
            "(skeleton only — run `dev wiki update` for LLM-enriched prose)."
        )
    except Exception as exc:
        # The wiki is a convenience layer; never let it fail `dev map`.
        logger.warning("Wiki refresh after map failed: %s", exc)
