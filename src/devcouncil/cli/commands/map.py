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
            "Code graph: `.devcouncil/graph/code_graph.json` (symbol-level; query with `dev graph`).",
            *wiki_lines,
            "",
            "Workflow for agents:",
            "1. Open `.devcouncil/repo_map.json` before guessing at file locations.",
            "2. Use the `files` list to resolve module ownership and nearby siblings.",
            "3. Use `subsystems` for subsystem-level navigation.",
            "4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.",
            "5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).",
            "6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.",
            "7. Prefer `dev graph dead --confidence extracted` + file greps for dead code. "
            "Treat `inferred` as unconfirmed. If `entry_roots` are empty / "
            "`liveness_unreachable_unreliable`, ignore `unreachable_files` and mass inferred dead. "
            "Check `unwired_candidates` / `dead_symbol_candidates` before creating new modules — "
            "wire what you create into a real caller.",
            "8. Use `dev graph query <name>` / `dev graph trace <a> <b>` / `dev graph dead` "
            "for symbol callers, paths, and dead-code tiers; `dev graph html` for the visualizer.",
            "9. Run `dev map` (or rely on post-tool-use auto-refresh) after large refactors.",
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
        text = _agent_guide_text(repo_map_path, repo_root, repo_map) + "\n"
        # Skip rewrite when unchanged so content_fingerprint (size+mtime) stays stable
        # across consecutive identical `dev map` runs.
        if path.exists() and path.read_text(encoding="utf-8") == text:
            continue
        path.write_text(text, encoding="utf-8")


def generate_map_artifacts(
    root: Path,
    output: Path,
    goal: str = "",
    *,
    scan_dependencies: bool = False,
    liveness: bool = True,
    lsp_refs: bool = False,
    quiet: bool = False,
) -> RepoMap:
    """Build the repo map and write repo_map.json + agent guides (no LLM, no re-init).

    Assumes ``.devcouncil/`` already exists. Shared by the ``dev map`` command and
    by project initialization so a freshly set-up repo is immediately navigable.
    ``scan_dependencies`` is opt-in (off for init and default mapping) because it can
    shell out to dependency auditors.
    ``lsp_refs`` opts into live LSP confirmation of dead-symbol candidates.
    ``quiet`` suppresses stderr status lines (for ``dev status --json`` auto-init).
    """
    import time

    t0 = time.perf_counter()
    repo_map = RepoMapper(root).map_repo(
        goal,
        scan_dependencies=scan_dependencies,
        liveness=liveness,
        lsp_refs=lsp_refs,
    )
    elapsed = time.perf_counter() - t0
    if not quiet:
        status_console.print(f"[dim]map completed in {elapsed:.2f}s[/dim]")
    graph_context = CodeReviewGraphAdapter(root).get_context()
    output = output if output.is_absolute() else root / output
    output.parent.mkdir(parents=True, exist_ok=True)
    write_model_json(output, repo_map)
    _write_agent_guides(root, output, repo_map)
    # Agent guides can add/change tracked files after the fingerprint was taken;
    # re-stamp so the on-disk map is not immediately stale for checkout/verify.
    mapper = RepoMapper(root)
    try:
        files = mapper.get_git_files()
        repo_map.generated_head = mapper._git_head()
        repo_map.indexed_hash = mapper._files_fingerprint(files)
        repo_map.content_fingerprint = mapper._content_fingerprint(files)
        write_model_json(output, repo_map)
    except Exception:
        logger.debug("Failed to re-stamp map fingerprints after agent guides", exc_info=True)
    if graph_context.available:
        graph_output = output.with_name("code_review_graph_context.json")
        write_model_json(graph_output, graph_context)
        if not quiet:
            status_console.print(f"[green]Wrote code-review-graph context to {graph_output}[/green]")
    return repo_map


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
            "warning: entry_roots empty — unreachable_files unreliable "
            "(liveness_unreachable_unreliable); ignore unreachable / mass inferred dead",
            file=sys.stderr,
        )
    samples = repo_map.unwired_candidates[:3] + repo_map.dead_symbol_candidates[:2]
    sample_txt = (", " + ", ".join(samples)) if samples else ""
    return (
        f"liveness: {len(repo_map.entry_roots)} entry roots, "
        f"{len(repo_map.unwired_candidates)} unwired, "
        f"{len(repo_map.unreachable_files)} unreachable, "
        f"{len(repo_map.dead_symbol_candidates)} dead symbols"
        f"{sample_txt}"
        " (map clears on any non-test importer; verify requires a pre-existing caller)"
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
):
    """Build the deterministic repository map without calling an LLM."""
    root = project_root.expanduser().resolve()
    # Reject missing roots before set_log_dir / initialize_project mkdir(parents=True)
    # silently creates an empty project and maps zero files with exit 0.
    if not root.is_dir():
        status_console.print(f"[red]Project root does not exist: {root}[/red]")
        raise typer.Exit(code=1)
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
        repo_map = generate_map_artifacts(
            root,
            output,
            goal,
            scan_dependencies=scan_deps,
            liveness=liveness,
            lsp_refs=use_lsp,
        )
        typer.echo(dump_json(repo_map.model_dump(), indent=2))
        status_console.print(f"[green]Wrote repository map to {output}[/green]")
        graph_out = root / ".devcouncil" / "graph" / "code_graph.json"
        if graph_out.is_file():
            status_console.print(f"[green]Wrote code graph to {graph_out}[/green]")
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
        from devcouncil.cli.commands.wiki import wiki_dir_for
        from devcouncil.knowledge.wiki import (
            _project_name,
            generate_wiki,
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
