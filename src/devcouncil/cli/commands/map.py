import json
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

app = typer.Typer(
    help=(
        "Build the repository map, query the code graph, and related HTML visualizers. "
        "`dev graph` is a compatibility alias for this command group."
    ),
)
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
            "dead_symbol_candidates and `dev map dead --confidence extracted`",
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


@app.callback(invoke_without_command=True)
def map_repo(
    ctx: typer.Context,
    goal: str = typer.Option(
        "",
        "--goal",
        help="Goal text used for candidate-file ranking (was a positional argument).",
    ),
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
    full: bool = typer.Option(
        False,
        "--full",
        help=(
            "Force a full isolated rebuild. Without it, a small change set since the "
            "last committed generation is applied incrementally."
        ),
    ),
    pdg: bool = typer.Option(
        False,
        "--pdg/--no-pdg",
        help="Build opt-in PDG/CFG/taint layer after map (Python-only, intra-procedural).",
    ),
):
    """Build the deterministic repository map without calling an LLM."""
    import sys

    if ctx.info_name == "graph":
        # TTY-only so `dev graph ... --json` stays machine-parseable under CliRunner
        # (which mixes stderr into `.output` by default).
        if sys.stderr.isatty():
            print(
                "note: `dev graph` is a compatibility alias; prefer `dev map ...`",
                file=sys.stderr,
            )
    if ctx.invoked_subcommand is not None:
        return
    root = project_root.expanduser().resolve()
    # Reject missing roots before set_log_dir / initialize_project mkdir(parents=True)
    # silently creates an empty project and maps zero files with exit 0.
    if not root.is_dir():
        status_console.print(f"[red]Project root does not exist: {root}[/red]")
        raise typer.Exit(code=1)
    # `dev map --goal /some/repo` with default project_root still maps CWD — surface intent.
    if goal and project_root == Path("."):
        goal_path = Path(goal).expanduser()
        if goal_path.is_absolute() and goal_path.is_dir() and goal_path.resolve() != root:
            status_console.print(
                f"[yellow]Goal option is a directory ({goal}). Did you mean "
                f"`dev map --project-root {goal}`? Mapping {root} with it as goal text.[/yellow]"
            )
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)

    # --- The Rust kernel is the map engine. There is no Python fallback. ---
    #
    # `devcouncil.indexing` and the Rust kernel answer the same questions
    # differently, and a fallback gives no signal which one answered. That is
    # how SC23 stayed hidden for a whole pass: every "hybrid" consumer raised,
    # silently took the Python path, and reported success. Any failure below
    # ends the stage red.
    import json as _json
    import time as _time

    from devcouncil.devmap_engine import DevMapEngineError, build_map

    def _map_payload() -> dict:
        try:
            return _json.loads(output.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {}

    def _is_stale() -> bool:
        # Unreadable or absent map is stale, never "fresh" — a check that could
        # not run must not report what a passing check reports.
        payload = _map_payload()
        if not payload:
            return True
        try:
            # Module-level `RepoMapper` on purpose: tests monkeypatch
            # `map_cmd.RepoMapper.map_is_stale`, and a function-local import
            # would rebind past the patch and silently ignore it.
            return bool(RepoMapper(project_root=root).map_is_stale(payload))
        except Exception:
            return True

    def _build_once() -> None:
        written = build_map(root, output=output)
        payload = _map_payload()
        try:
            typer.echo(dump_json(payload, indent=2))
        except BrokenPipeError:
            import os as _os
            import sys as _sys

            logger.debug("stdout pipe closed while streaming repo map JSON")
            try:
                _os.dup2(_os.open(_os.devnull, _os.O_WRONLY), _sys.stdout.fileno())
            except OSError:
                pass
        status_console.print(f"[green]Wrote repository map to {written}[/green]")

    try:
        if if_stale and not _is_stale():
            # Keeps the message the `--if-stale` contract already published;
            # consumers and tests match on "Map is fresh".
            status_console.print(f"[dim]Map is fresh; skipping rebuild ({output})[/dim]")
            raise typer.Exit(code=0)
        _build_once()
        if watch:
            # Through the existing `_watch_map` seam, not an inline loop: the
            # loop bypassed the very hook `test_map_watch_flag_invokes_watch_map`
            # monkeypatches, so under a test runner it never returned and the
            # suite hung instead of failing.
            _watch_map(root, liveness=liveness)
    except DevMapEngineError as exc:
        status_console.print(f"[red]devmap (Rust) could not build the map: {exc}[/red]")
        raise typer.Exit(code=1) from exc
    raise typer.Exit(code=0)
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
        from devcouncil.codeintel.build_control import (
            GraphBuildBusy,
            GraphBuildFailed,
            GraphBuildTimeout,
        )

        try:
            from devcouncil.indexing.map_artifacts import refresh_map_artifacts

            refresh = refresh_map_artifacts(
                root,
                output,
                goal,
                scan_dependencies=scan_deps,
                liveness=liveness,
                lsp_refs=use_lsp,
                full=full,
            )
            repo_map = refresh.repo_map
        except GraphBuildBusy as exc:
            status_console.print(f"[red]{exc}[/red]")
            status_console.print(
                "[dim]Recover with `dev map unlock` (add --force if the holder is "
                "wedged but still reporting progress).[/dim]"
            )
            raise typer.Exit(code=1) from exc
        except (GraphBuildTimeout, GraphBuildFailed) as exc:
            # These reached the CLI as an uncaught traceback + CRITICAL log. Print
            # the failure and the three things that actually unblock a big repo.
            logger.warning("dev map: graph build did not complete", exc_info=True)
            status_console.print(f"[red]Graph build did not complete: {exc}[/red]")
            status_console.print(
                "[dim]Try: `dev map unlock` to free a stuck writer; `dev map "
                "--no-liveness` to skip the liveness pass; or raise "
                "indexing.build_stall_timeout_seconds / "
                "indexing.build_total_timeout_seconds in .devcouncil/config.yaml.[/dim]"
            )
            raise typer.Exit(code=1) from exc
        if refresh.build_incomplete:
            status_console.print(
                f"[yellow]Graph build did not finish; refreshed the map from the last "
                f"committed generation ({refresh.reason}).[/yellow]"
            )
            raise typer.Exit(code=1)
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
                    "run `dev map doctor`[/yellow]"
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


@app.command("html")
def map_html(
    ctx: typer.Context,
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    open_browser: bool = typer.Option(False, "--open", help="Open in the default browser."),
    symbols: bool = typer.Option(
        False,
        "--symbols",
        help=(
            "Write the symbol-level graph visualizer instead of the subsystem map "
            "(same as `dev map graph-html`). `dev graph html` always uses that visualizer."
        ),
    ),
) -> None:
    """Subsystem map HTML (`dev map html`) or symbol graph (`dev graph html` / `--symbols`)."""
    parent = (ctx.parent.info_name if ctx.parent else "") or ""
    if parent == "graph" or symbols:
        from devcouncil.cli.commands.graph_cmd import graph_html

        graph_html(
            project_root=project_root,
            open_browser=open_browser,
            symbols=symbols,
        )
        return
    from devcouncil.indexing.map_viz import write_map_html

    root = project_root.expanduser().resolve()
    try:
        out = write_map_html(root, open_browser=open_browser)
    except FileNotFoundError as exc:
        status_console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    status_console.print(f"[green]Wrote {out}[/green]")


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
    """Rebuild through the Rust kernel whenever the repository fingerprint moves.

    Polls exactly the fingerprint `--if-stale` reads, so a watch rebuild and an
    `--if-stale` rebuild fire on the same evidence rather than on two rules that
    can disagree. Ctrl-C is a clean stop, not a failure.
    """
    import time

    from devcouncil.devmap_engine import DevMapEngineError, build_map

    del liveness  # the Rust kernel always computes liveness; no partial mode
    status_console.print("[cyan]Watching for changes (Ctrl-C to stop)…[/cyan]")
    try:
        while True:
            time.sleep(2.0)
            payload: dict = {}
            map_path = root / ".devcouncil" / "repo_map.json"
            try:
                payload = json.loads(map_path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                payload = {}
            stale = True
            if payload:
                try:
                    stale = bool(RepoMapper(project_root=root).map_is_stale(payload))
                except Exception:
                    stale = True
            if not stale:
                continue
            try:
                build_map(root)
                status_console.print("[green]Map refreshed.[/green]")
            except DevMapEngineError as exc:
                # A failed rebuild must not end the watch — but it must be
                # visible, not swallowed, or the watcher looks alive while
                # serving an increasingly stale map.
                status_console.print(f"[red]devmap rebuild failed: {exc}[/red]")
    except KeyboardInterrupt:
        status_console.print("\n[cyan]Stopped watching.[/cyan]")


def _mount_graph_commands(target: typer.Typer) -> None:
    """Attach the graph command tree onto the shared map Typer.

    ``html`` stays the subsystem visualizer on this app; the symbol visualizer is
    registered as ``graph-html``. When the same app is dual-mounted as ``graph``,
    ``map_html`` dispatches to the symbol visualizer via ``ctx.parent.info_name``.
    """
    from typer.models import CommandInfo, DefaultPlaceholder

    from devcouncil.cli.commands import graph_cmd

    # A partially-initialised graph_cmd (import cycle, or a stale editable
    # install being rewritten under a running process) used to raise
    # ``AttributeError: module 'graph_cmd' has no attribute 'app'`` at import
    # time and take the whole CLI down — including `dev map unlock`, the command
    # you need precisely when a build has just been killed. Degrade instead.
    source_app = getattr(graph_cmd, "app", None)
    if source_app is None:
        logger.warning(
            "graph command group unavailable; `dev map <graph subcommand>` is not "
            "mounted this run — use `dev graph ...` (e.g. `dev graph unlock`)"
        )
        return

    existing_cmds = {c.name for c in target.registered_commands if c.name}
    existing_groups: set[str] = set()
    for group in target.registered_groups:
        name = None if isinstance(group.name, DefaultPlaceholder) else group.name
        if name:
            existing_groups.add(name)

    for cmd in source_app.registered_commands:
        name = cmd.name
        if not name:
            continue
        if name == "html":
            if "graph-html" in existing_cmds:
                continue
            target.registered_commands.append(
                CommandInfo(
                    name="graph-html",
                    cls=cmd.cls,
                    context_settings=cmd.context_settings,
                    callback=cmd.callback,
                    help=(
                        "Write interactive .devcouncil/graph/graph.html "
                        "(symbol-level visualizer)."
                    ),
                    epilog=cmd.epilog,
                    short_help=cmd.short_help,
                    options_metavar=cmd.options_metavar,
                    add_help_option=cmd.add_help_option,
                    no_args_is_help=cmd.no_args_is_help,
                    hidden=cmd.hidden,
                    deprecated=cmd.deprecated,
                    rich_help_panel=cmd.rich_help_panel,
                )
            )
            existing_cmds.add("graph-html")
            continue
        if name in existing_cmds:
            continue
        target.registered_commands.append(cmd)
        existing_cmds.add(name)

    for group in source_app.registered_groups:
        gname = None if isinstance(group.name, DefaultPlaceholder) else group.name
        ti = group.typer_instance
        if isinstance(ti, DefaultPlaceholder) or ti is None or not gname:
            continue
        if gname in existing_groups:
            continue
        target.add_typer(ti, name=gname)
        existing_groups.add(gname)


_mount_graph_commands(app)
