import logging
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.indexing.map_artifacts import (
    _important_surfaces as _important_surfaces,
    _wiki_index_rel as _wiki_index_rel,
    generate_map_artifacts as generate_map_artifacts,
    write_agent_guides,
)
from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.storage.db import get_db
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

# Back-compat aliases for tests / external importers.
_write_agent_guides = write_agent_guides

console = Console()
status_console = Console(stderr=True)
logger = logging.getLogger(__name__)


def _liveness_summary(repo_map: RepoMap) -> str | None:
    unreliable = bool(getattr(repo_map, "liveness_unreachable_unreliable", False))
    if not (
        repo_map.entry_roots
        or repo_map.unwired_candidates
        or repo_map.unreachable_files
        or repo_map.dead_symbol_candidates
        or unreliable
    ):
        return None
    if not repo_map.entry_roots or unreliable:
        import sys

        print(
            "warning: unreachable_files low-confidence "
            "(liveness_unreachable_unreliable); prefer unwired_candidates / "
            "dead_symbol_candidates and `dev graph dead --confidence extracted`",
            file=sys.stderr,
        )
    samples = repo_map.unwired_candidates[:3] + repo_map.dead_symbol_candidates[:2]
    sample_txt = (", " + ", ".join(samples)) if samples else ""
    unreachable_txt = (
        "omitted (unreliable)"
        if unreliable
        else str(len(repo_map.unreachable_files))
    )
    return (
        f"liveness: {len(repo_map.entry_roots)} entry roots, "
        f"{len(repo_map.unwired_candidates)} unwired, "
        f"{unreachable_txt} unreachable, "
        f"{len(repo_map.dead_symbol_candidates)} dead symbols"
        f"{sample_txt}"
        " (prefer unwired + extracted dead; map clears on any non-test importer)"
    )


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
    liveness: bool = typer.Option(
        True,
        "--liveness/--no-liveness",
        help="Compute entry_roots / unwired / unreachable / dead_symbol candidate lists (on by default).",
    ),
    lsp_refs: bool = typer.Option(
        False,
        "--lsp-refs/--no-lsp-refs",
        help=(
            "Confirm dead-symbol candidates via live LSP references when a language "
            "server is on PATH. Also set indexing.lsp_refs in config.yaml. Off by default."
        ),
    ),
    refresh_wiki: bool = typer.Option(
        True,
        "--wiki/--no-wiki",
        help="After mapping, refresh stale codebase-wiki page skeletons when a wiki exists (no LLM calls).",
    ),
    watch: bool = typer.Option(
        False,
        "--watch",
        help="After the initial map, watch the tree and incrementally refresh on code edits (Ctrl-C to stop).",
    ),
    if_stale: bool = typer.Option(
        False,
        "--if-stale",
        help="Fingerprint-check first and exit 0 without rebuilding when the on-disk map is still fresh.",
    ),
    pdg: bool = typer.Option(
        False,
        "--pdg/--no-pdg",
        help="Build opt-in PDG/CFG/taint layer after map (Python-only, intra-procedural).",
    ),
):
    """Build the deterministic repository map without calling an LLM."""
    root = project_root.expanduser().resolve()
    # Reject missing roots before set_log_dir / initialize_project mkdir(parents=True)
    # silently creates an empty project and maps zero files with exit 0.
    if not root.is_dir():
        status_console.print(f"[red]Project root does not exist: {root}[/red]")
        raise typer.Exit(code=1)
    # `dev map /some/repo` parses the path as the GOAL and maps the CWD repo with
    # exit 0 — surface the likely intent instead of silently mapping the wrong repo.
    if goal and project_root == Path("."):
        goal_path = Path(goal).expanduser()
        if goal_path.is_absolute() and goal_path.is_dir() and goal_path.resolve() != root:
            status_console.print(
                f"[yellow]Goal argument is a directory ({goal}). Did you mean "
                f"`dev map --project-root {goal}`? Mapping {root} with it as goal text.[/yellow]"
            )
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    # CLI flag OR config; flag alone is enough without rewriting config.
    use_lsp = lsp_refs
    if not use_lsp:
        try:
            from devcouncil.app.config import load_config

            use_lsp = bool(load_config(root).indexing.lsp_refs)
        except Exception:
            use_lsp = False
    logger.info(
        "dev map: goal=%r scan_deps=%s liveness=%s lsp_refs=%s if_stale=%s",
        goal, scan_deps, liveness, use_lsp, if_stale,
    )
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(root, quiet=True, with_map=False)
    if not get_db(root):
        raise typer.Exit(code=1)

    output = output if output.is_absolute() else root / output
    if if_stale and output.is_file():
        try:
            from devcouncil.utils.json_persist import read_json

            data = read_json(output) or {}
            if isinstance(data, dict) and not RepoMapper(root).map_is_stale(data):
                status_console.print(f"[dim]Map is fresh; skipping rebuild ({output})[/dim]")
                raise typer.Exit(code=0)
        except typer.Exit:
            raise
        except Exception:
            logger.debug("if-stale freshness check failed; rebuilding", exc_info=True)

    with log_stage("map", project_root=root, scan_deps=scan_deps):
        log_step("map/1: generating repository map", project_root=root, trace=True)
        from devcouncil.codeintel.build_control import GraphBuildBusy

        try:
            from devcouncil.indexing.map_artifacts import refresh_map_artifacts

            refresh = refresh_map_artifacts(
                root,
                output,
                goal,
                scan_dependencies=scan_deps,
                liveness=liveness,
                lsp_refs=use_lsp,
            )
            repo_map = refresh.repo_map
        except GraphBuildBusy as exc:
            status_console.print(f"[red]{exc}[/red]")
            raise typer.Exit(code=1) from exc
        if refresh.degraded:
            status_console.print(
                f"[red]Map wrote lean/degraded artifacts: {refresh.reason or refresh.mode}[/red]"
            )
            raise typer.Exit(code=1)
        if refresh.compatibility_export_degraded:
            status_console.print(
                f"[yellow]Compatibility export degraded (canonical SQLite ok): "
                f"{refresh.reason}[/yellow]"
            )
        try:
            typer.echo(dump_json(repo_map.model_dump(), indent=2))
        except BrokenPipeError:
            # Consumer closed stdout early (`dev map | head`). The artifacts are
            # already written — a closed pipe must not turn the stage red.
            import os
            import sys

            logger.debug("stdout pipe closed while streaming repo map JSON")
            try:
                os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
            except OSError:
                pass
        status_console.print(f"[green]Wrote repository map to {output}[/green]")
        graph_out = root / ".devcouncil" / "graph" / "code_graph.json"
        if not graph_out.is_file():
            # The export write failed or an external cleanup removed the JSON
            # while SQLite (canonical) still holds the graph — re-export instead
            # of exiting green without the documented artifact.
            from devcouncil.indexing.graph.build import export_code_graph_json

            if export_code_graph_json(root) is not None:
                status_console.print(
                    f"[yellow]Code graph JSON was missing; re-exported from store to {graph_out}[/yellow]"
                )
            else:
                status_console.print(
                    f"[yellow]Code graph JSON missing and store re-export failed ({graph_out}); "
                    "run `dev graph doctor`[/yellow]"
                )
        if graph_out.is_file():
            status_console.print(f"[green]Wrote code graph to {graph_out}[/green]")
            if pdg:
                try:
                    from devcouncil.indexing.graph.build import (
                        build_pdg_for_paths,
                        load_code_graph,
                        merge_pdg_into_graph,
                        write_code_graph,
                    )

                    graph = load_code_graph(root)
                    if graph is not None:
                        layer = build_pdg_for_paths(root, graph)
                        shards = merge_pdg_into_graph(graph, layer)
                        merged: dict = {}
                        try:
                            from devcouncil.codeintel import get_codeintel_service

                            merged = dict(get_codeintel_service(root).store.analysis_shards())
                        except Exception:
                            pass
                        for path, payload in shards.items():
                            merged.setdefault(path, {}).update(payload)
                        write_code_graph(root, graph, analysis_shards=merged)
                        stats = (graph.meta.get("pdg") or {}).get("stats") or {}
                        status_console.print(
                            f"[green]Wrote PDG layer[/green] "
                            f"({stats.get('function_count', 0)} functions, "
                            f"{stats.get('taint_count', 0)} taint findings)"
                        )
                except Exception as exc:
                    logger.warning("PDG build after map failed: %s", exc)
            try:
                from devcouncil.app.config import load_config

                if bool(load_config(root).indexing.write_graph_html):
                    from devcouncil.indexing.viz import write_graph_html

                    html_out = write_graph_html(root, open_browser=False)
                    status_console.print(f"[green]Wrote graph HTML to {html_out}[/green]")
            except Exception as exc:
                logger.warning("Failed to write graph.html after map: %s", exc)
        summary = _liveness_summary(repo_map)
        if summary:
            status_console.print(f"[cyan]{summary}[/cyan]")
        if refresh_wiki:
            _refresh_wiki_skeletons(root, repo_map)
        log_step("map/complete", project_root=root, trace=True)
        if watch:
            _watch_map(root, liveness=liveness)


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
        from devcouncil.knowledge.wiki import (
            _project_name,
            generate_wiki,
            wiki_dir_for,
            wiki_stale_pages,
        )

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


def _watch_map(root: Path, *, liveness: bool = True) -> None:
    """Run the shared code-intelligence coordinator for map compatibility."""
    import time

    from devcouncil.codeintel.sync import get_sync_coordinator
    from devcouncil.indexing.graph.build import refresh_map_for_paths

    coordinator = get_sync_coordinator(
        root,
        debounce_seconds=0.8,
        sync_callback=lambda paths: refresh_map_for_paths(root, paths, liveness=liveness),
    )
    state = coordinator.start()
    status_console.print(
        f"[cyan]Watching {root} with {state.backend or 'reconciliation'} "
        f"(state={state.state}, debounce 0.8s). Ctrl-C to stop.[/cyan]"
    )
    try:
        while True:
            time.sleep(0.8)
            before = coordinator.status().pending
            if before and not coordinator.sync_now():
                failure = coordinator.status().last_error or coordinator.status().degraded_reason
                status_console.print(f"[yellow]Watch refresh failed (ignored): {failure}[/yellow]")
            elif before:
                status_console.print(f"[green]Refreshed map for {len(before)} path(s)[/green]")
    except KeyboardInterrupt:
        status_console.print("Stopped watching.")
    finally:
        coordinator.stop(timeout=2)
