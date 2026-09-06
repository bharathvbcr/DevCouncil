import json
import logging
import sys
import threading
from collections.abc import Mapping
from pathlib import Path
from typing import TYPE_CHECKING

import typer
from rich.console import Console

from devcouncil.indexing.map_artifacts import (
    _important_surfaces as _important_surfaces,
    _wiki_index_rel as _wiki_index_rel,
    generate_map_artifacts as generate_map_artifacts,
    write_agent_guides,
)
from devcouncil.indexing.repo_mapper import RepoMap, RepoMapper
from devcouncil.utils.json_persist import dump_json

if TYPE_CHECKING:  # pragma: no cover - resolved by `__getattr__` at runtime.
    # Type checkers see the attribute the PEP 562 hook below provides; ruff
    # cannot, so this import is "unused" to it and load-bearing to mypy.
    from devcouncil.integrations.code_review_graph import (  # noqa: F401
        CodeReviewGraphAdapter,
    )

# Back-compat aliases for tests / external importers.
_write_agent_guides = write_agent_guides


def __getattr__(name: str) -> object:
    """Resolve `CodeReviewGraphAdapter` on first access, not at import.

    `devcouncil.integrations.code_review_graph` costs 62 ms to import and is
    reached by exactly one subcommand out of this module's many, so paying for
    it on every `dev map` is waste. A function-local import would also have
    avoided that, but it would have *removed* the attribute: this name is the
    seam four tests substitute to drive `dev graph-context`'s output branches,
    and a seam that silently stops being one is worse than the import cost.
    PEP 562 keeps it a real, patchable module attribute that costs nothing
    until something asks for it.
    """
    if name == "CodeReviewGraphAdapter":
        from devcouncil.integrations.code_review_graph import (
            CodeReviewGraphAdapter as adapter,
        )

        return adapter
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

app = typer.Typer(
    help=(
        "Build the repository map, query the code graph, and related HTML visualizers. "
        "`dev graph` is a compatibility alias for this command group."
    ),
)
console = Console()
status_console = Console(stderr=True)
logger = logging.getLogger(__name__)


def _disclosed_count(meta: object, key: str, shown: int) -> tuple[str, bool]:
    """Render ``shown`` against the total the map disclosed for ``key``.

    Every liveness list is capped by the kernel before it reaches the artifact,
    and the cap leaves no trace in the list itself: this repository's own map
    carries 200 unwired candidates out of 540 and 200 dead symbols out of 208.
    ``liveness_meta`` has always recorded ``{shown, total, truncated}`` per
    bucket (`devmap-query/src/manifest.rs`); it was this line — the one an agent
    reads before concluding "that is all of them" — that dropped the
    denominator and showed a capped sample as complete coverage.

    Returns the rendered count and whether it is a *floor* rather than a total.
    A disclosure that its own list disproves (``total`` below the number of
    entries actually present, or not an integer) is treated as no disclosure:
    printing an authoritative-looking total that the artifact contradicts is
    the failure this function exists to prevent, so the incoherent case falls
    back to the floor form instead of trusting either number.
    """
    total: object = None
    if isinstance(meta, Mapping):
        bucket = meta.get(key)
        if isinstance(bucket, Mapping):
            total = bucket.get("total")
    if isinstance(total, int) and not isinstance(total, bool) and total >= shown:
        return (f"{shown} of {total}" if total > shown else str(shown)), False
    # An empty list is not a truncated sample of anything, so it needs no floor
    # marker; a non-empty one with no total behind it is a lower bound only.
    return (f"{shown}+" if shown else "0"), bool(shown)


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
    meta = getattr(repo_map, "liveness_meta", None)
    roots_txt, roots_floor = _disclosed_count(meta, "entry_roots", len(repo_map.entry_roots))
    unwired_txt, unwired_floor = _disclosed_count(
        meta, "unwired", len(repo_map.unwired_candidates)
    )
    dead_txt, dead_floor = _disclosed_count(
        meta, "dead_symbol", len(repo_map.dead_symbol_candidates)
    )
    notes = ["prefer unwired + extracted dead"]
    # `unwired.total` counts only the files the question could be asked of: a
    # file whose imports never extracted cannot answer "is anything importing
    # me", so the kernel drops it from the population rather than calling it
    # unwired. Excluding it is right; not saying so would make an unexamined
    # file indistinguishable from an examined one that came back clean.
    excluded = _excluded_coverage_loss(meta)
    if excluded:
        notes.append(f"{excluded} files excluded from unwired: extraction coverage loss")
    if roots_floor or unwired_floor or dead_floor:
        notes.append("N+ means this map disclosed no total")
    notes.append("map clears on any non-test importer")
    return (
        f"liveness: {roots_txt} entry roots, "
        f"{unwired_txt} unwired, "
        f"{unreachable_txt} unreachable, "
        f"{dead_txt} dead symbols"
        f"{sample_txt}"
        f" ({'; '.join(notes)})"
    )


def _excluded_coverage_loss(meta: object) -> int:
    """Files the unwired question could not be asked of, per the map's own count."""
    if not isinstance(meta, Mapping):
        return 0
    bucket = meta.get("unwired")
    if not isinstance(bucket, Mapping):
        return 0
    excluded = bucket.get("excluded_coverage_loss")
    if isinstance(excluded, int) and not isinstance(excluded, bool) and excluded > 0:
        return excluded
    return 0


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
        help=(
            "Refresh only. Fingerprint-check first and exit 0 without rebuilding when "
            "the on-disk map is still fresh — and also when there is no map at all, "
            "since a cold build is a full index rather than a refresh. Safe to wire "
            "into an editor or git hook; plain `dev map` is not."
        ),
    ),
    full: bool = typer.Option(
        False,
        "--full",
        help=(
            "Force a full cold rebuild in the kernel. Without it, an unchanged tree is "
            "a no-op and a changed one is rebuilt incrementally."
        ),
    ),
    pdg: bool = typer.Option(
        False,
        "--pdg/--no-pdg",
        help="Build opt-in PDG/CFG/taint layer after map (Python-only, intra-procedural).",
    ),
):
    """Build the deterministic repository map without calling an LLM.

    The Rust kernel is the map engine and the only writer of `repo_map.json`
    and `code_graph.json`. There is no Python fallback: two engines answering
    the same question differently, with no signal which one answered, is how a
    stale map comes to look like a fresh one. Anything the kernel does not do —
    goal ranking, dependency auditing, agent guides, the wiki, the PDG layer —
    is layered on top of its artifacts here, never computed by a second index.

    Flags the kernel cannot honour are gone rather than ignored: `--no-liveness`
    (the kernel always computes liveness) and `--lsp-refs` (the LSP adjunct was
    cut with the Python engine). A flag that is accepted and does nothing is
    worse than one that is rejected.
    """
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
    logger.info(
        "dev map: goal=%r scan_deps=%s if_stale=%s full=%s pdg=%s",
        goal, scan_deps, if_stale, full, pdg,
    )

    enclosing = _enclosing_project_root(root)
    if enclosing is not None:
        # A `.devcouncil/config.yaml` makes its directory a project root, and
        # the root is just `--project-root` (default: cwd). So an agent that
        # cd's into a subdirectory to run its build or tests, in a repo that has
        # a nested config, silently indexes that subtree as a separate project.
        #
        # Observed: three nested configs committed by accident into a polyglot
        # repo's `backend/*/` directories produced a 713MB duplicate index of
        # one subtree, rebuilt on its own schedule, alongside the real 1.7GB
        # root index. Nothing reported it, because from inside the subdirectory
        # nothing is wrong.
        status_console.print(
            f"[yellow]Nested project root.[/yellow] {root} sits inside "
            f"{enclosing}, which is also a DevCouncil project.\n"
            "Indexing here builds a SECOND, separate index of this subtree — it "
            "does not contribute to the parent's map.\n"
            f"If that is not what you want, run from {enclosing} "
            f"(or pass --project-root {enclosing}) and delete "
            f"{root / '.devcouncil' / 'config.yaml'}."
        )

    from devcouncil.cli.commands.init import initialize_project

    initialize_project(root, quiet=True, with_map=False)
    # Imported at the point of use. Commands are built lazily
    # (`cli/main.py`), so a module-scope import here is paid by every
    # `dev map` — and `storage.db` pulls SQLAlchemy and SQLModel, ~85 ms
    # measured, for one existence check.
    from devcouncil.storage.db import get_db

    if not get_db(root):
        raise typer.Exit(code=1)

    output = output if output.is_absolute() else root / output
    if if_stale:
        if not output.is_file():
            # `--if-stale` means "refresh only if there is something cheap to
            # refresh". With no map on disk there is nothing to compare against,
            # and the only way to proceed is the most expensive operation this
            # tool has: a full cold build, which on a large polyglot repo is
            # several hundred megabytes of SQLite and many minutes of CPU.
            #
            # This used to fall straight through to that build with no message.
            # `.devcouncil/` is gitignored, so every fresh git worktree starts
            # with no map — and an editor hook wired to `dev map --if-stale`
            # therefore kicked off a full build on the first file edit. Hooks
            # get killed on timeout, but the grandchild build is not in the
            # hook's process group and survives as an orphan at PPID 1, holding
            # a core until it finishes. Observed on one machine: 89 abandoned
            # build and shell processes, load average 184, 0.0% idle.
            #
            # Refusing here is the honest reading of the flag. A caller who
            # wants an unconditional build runs `dev map` without it.
            status_console.print(
                f"[yellow]No map at {output} — nothing to refresh.[/yellow]\n"
                "[yellow]--if-stale will not start a cold build: that is a full "
                "index, not a refresh.[/yellow]\n"
                "Run [bold]dev map[/bold] (without --if-stale) to build one, in the "
                "foreground where you can see it."
            )
            raise typer.Exit(code=0)
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

    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    def _build_once() -> None:
        refresh = refresh_map_artifacts(
            root,
            output,
            goal,
            scan_dependencies=scan_deps,
            full=full,
            quiet=True,
        )
        # Echo the artifact as written, not a re-serialization of the model: the
        # kernel writes fields the Python model does not declare, and a reader
        # piping `dev map` must see the same bytes `repo_map.json` holds.
        try:
            payload = json.loads(output.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            payload = refresh.repo_map.model_dump()
        try:
            typer.echo(dump_json(payload, indent=2))
        except BrokenPipeError:
            # Consumer closed stdout early (`dev map | head`). The artifacts are
            # already written — a closed pipe must not turn the stage red.
            import os as _os

            logger.debug("stdout pipe closed while streaming repo map JSON")
            try:
                _os.dup2(_os.open(_os.devnull, _os.O_WRONLY), sys.stdout.fileno())
            except OSError:
                pass
        status_console.print(f"[green]Wrote repository map to {output}[/green]")
        graph_out = root / ".devcouncil" / "graph" / "code_graph.json"
        if graph_out.is_file():
            status_console.print(f"[green]Wrote code graph to {graph_out}[/green]")
            if pdg:
                _build_pdg_layer(root)
            _write_graph_html_if_configured(root)
        kernel = getattr(refresh, "kernel_status", None)
        if kernel is not None and (not kernel.is_fresh or kernel.degraded_reason):
            # `devmap status` said `is_fresh: false, 64 quarantined` for days
            # while this command printed a green line. A build that leaves the
            # store degraded says so, in the same breath, with the way out.
            status_console.print(
                "[yellow]Kernel store is not fresh after this build: "
                f"{kernel.degraded_reason or 'pending paths remain'} "
                f"(pending {kernel.pending_count}, quarantined {kernel.quarantined_count}). "
                "Files the kernel could not index are absent from the graph — see "
                "`dev map status`; `dev map repair --pending` drops stuck entries.[/yellow]"
            )
        summary = _liveness_summary(refresh.repo_map)
        if summary:
            status_console.print(f"[cyan]{summary}[/cyan]")
        if refresh_wiki:
            _refresh_wiki_skeletons(root, refresh.repo_map)

    from devcouncil.telemetry.stages import log_stage, log_step

    try:
        with log_stage("map", project_root=root, scan_deps=scan_deps, full=full):
            log_step("map/1: building through the devmap kernel", project_root=root, trace=True)
            _build_once()
            log_step("map/complete", project_root=root, trace=True)
        if watch:
            # Through the existing `_watch_map` seam, not an inline loop: the
            # loop bypassed the very hook `test_map_watch_flag_invokes_watch_map`
            # monkeypatches, so under a test runner it never returned and the
            # suite hung instead of failing.
            _watch_map(root)
    except DevMapEngineError as exc:
        # The code is what an agent branches on, the fix is what it runs, and
        # the run id is where the full record lives. A bare message was all
        # three folded into prose nobody could act on without reading it.
        # markup=False: `[store_locked]` is a diagnosis code, and Rich would
        # otherwise read it as a style tag and print nothing where it stood.
        status_console.print(
            f"devmap (Rust) could not build the map [{exc.code}]: {exc}",
            style="red",
            markup=False,
            highlight=False,
        )
        if exc.fix:
            status_console.print(f"fix: {exc.fix}", markup=False, highlight=False)
        if exc.run_id:
            status_console.print(
                f"run: {exc.run_id} — `dev map runs --last 1 --json` has the record",
                markup=False,
                highlight=False,
            )
        raise typer.Exit(code=1) from exc
    raise typer.Exit(code=0)


def _build_pdg_layer(root: Path) -> None:
    """Opt-in PDG/CFG/taint layer, computed by the Python analyser over the
    kernel's graph and merged into the JSON export. Failure is reported, never
    fatal: the map is already written."""
    try:
        from devcouncil.indexing.graph.build import (
            build_pdg_for_paths,
            load_code_graph,
            merge_pdg_into_graph,
            write_code_graph,
        )

        graph = load_code_graph(root)
        if graph is None:
            status_console.print("[yellow]PDG layer skipped: no code graph to analyse[/yellow]")
            return
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
        status_console.print(f"[yellow]PDG layer failed: {exc}[/yellow]")


def _write_graph_html_if_configured(root: Path) -> None:
    try:
        from devcouncil.app.config import load_config

        if bool(load_config(root).indexing.write_graph_html):
            from devcouncil.indexing.viz import write_graph_html

            html_out = write_graph_html(root, open_browser=False)
            status_console.print(f"[green]Wrote graph HTML to {html_out}[/green]")
    except Exception as exc:
        logger.warning("Failed to write graph.html after map: %s", exc)


def _enclosing_project_root(root: Path) -> Path | None:
    """Return the nearest ancestor that is also a DevCouncil project, if any.

    A project root is any directory holding ``.devcouncil/config.yaml``. Nesting
    one inside another is almost always accidental — a stray `dev init` in a
    subdirectory, or a config committed by a snapshot commit — and it is
    invisible from inside the child.
    """
    try:
        resolved = root.resolve()
    except OSError:
        return None
    for parent in resolved.parents:
        if (parent / ".devcouncil" / "config.yaml").is_file():
            return parent
    return None


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
    # The kernel renders it. See `devmap_engine.render_map_html` for why there
    # is no Python fallback: a second renderer of one artifact is one that
    # falls behind without saying so.
    from devcouncil.devmap_engine import DevMapEngineError, render_map_html

    root = project_root.expanduser().resolve()
    try:
        out = render_map_html(root)
    except DevMapEngineError as exc:
        status_console.print(f"[red]{exc}[/red]")
        if exc.fix:
            status_console.print(f"[yellow]{exc.fix}[/yellow]")
        raise typer.Exit(code=1) from exc
    status_console.print(f"[green]Wrote {out}[/green]")
    if open_browser:
        import webbrowser

        webbrowser.open(out.resolve().as_uri())


def graph_context_cmd(
    files: list[str] = typer.Option([], "--file", help="Changed files to scope blast-radius (repeatable)."),
    json_output: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Return code-review-graph context for the given files."""
    root = project_root.expanduser().resolve()
    # Read through the module object so `__getattr__` above runs and so a
    # substituted attribute is the one used. A `from … import` here would bind
    # the real class regardless of what a caller patched.
    adapter = sys.modules[__name__].CodeReviewGraphAdapter
    context = adapter(root).get_context(files)
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


#: Seconds between fingerprint checks when no filesystem event has arrived.
#: With a working observer this is only a safety net (an event the observer
#: missed — an overflow, a bind mount, a network filesystem) and can be long.
WATCH_POLL_INTERVAL_SECONDS = 30.0
#: Polling cadence when no observer could be started at all.
WATCH_FALLBACK_INTERVAL_SECONDS = 2.0
#: Quiet period after the first event of a burst before the fingerprint is
#: checked, so one `git checkout` costs one check rather than thousands.
WATCH_DEBOUNCE_SECONDS = 0.5

_WATCH_IGNORED_SEGMENTS = ("/.devcouncil/", "/.git/")


def _start_change_observer(root: Path, changed: threading.Event):
    """Start a filesystem observer that sets ``changed`` on any event under ``root``.

    Returns the observer, or ``None`` when one cannot be started — the caller
    then polls. Events under `.devcouncil/` (the artifacts a rebuild writes)
    and `.git/` (whose ref churn is already reflected by the working-tree
    events a checkout produces) are ignored so a rebuild does not wake itself.
    """
    try:
        from watchdog.events import FileSystemEventHandler
        from watchdog.observers import Observer
    except Exception:  # noqa: BLE001 - optional at runtime; polling covers it
        return None

    class _Handler(FileSystemEventHandler):
        def on_any_event(self, event) -> None:  # noqa: ANN001
            source = str(getattr(event, "src_path", "") or "").replace("\\", "/")
            if any(segment in source + "/" for segment in _WATCH_IGNORED_SEGMENTS):
                return
            changed.set()

    try:
        observer = Observer()
        observer.schedule(_Handler(), str(root), recursive=True)
        observer.daemon = True
        observer.start()
    except Exception:  # noqa: BLE001 - fall back to polling rather than fail the watch
        logger.debug("filesystem observer unavailable; polling instead", exc_info=True)
        return None
    return observer


def _wait_for_change(changed: threading.Event, timeout: float) -> bool:
    """Block until an event arrives or ``timeout`` elapses. Patch point for tests."""
    return changed.wait(timeout)


def _watch_map(root: Path, *, liveness: bool = True) -> None:
    """Rebuild through the Rust kernel whenever the repository fingerprint moves.

    Event-driven, not polled. The previous loop re-ran `map_is_stale` every two
    seconds — three git subprocesses and two stats per tracked file, measured at
    ~90 ms per tick on this repository (4.5% of a core, continuously) and
    extrapolated to several seconds per tick at 70,000 files, where the poll no
    longer fits in its own interval. Now a filesystem event wakes the loop, a
    short debounce coalesces the burst, and only then is the fingerprint read —
    still exactly the evidence `--if-stale` reads, so a watch rebuild and an
    `--if-stale` rebuild fire on the same rule. A slow poll remains as the
    safety net for events the observer misses. Ctrl-C is a clean stop.
    """
    import time

    from devcouncil.devmap_engine import DevMapEngineError, build_map

    del liveness  # the Rust kernel always computes liveness; no partial mode
    changed = threading.Event()
    observer = _start_change_observer(root, changed)
    interval = WATCH_POLL_INTERVAL_SECONDS if observer else WATCH_FALLBACK_INTERVAL_SECONDS
    if observer is None:
        status_console.print(
            "[yellow]No filesystem observer available; polling the fingerprint "
            f"every {interval:.0f}s instead.[/yellow]"
        )
    status_console.print("[cyan]Watching for changes (Ctrl-C to stop)…[/cyan]")
    try:
        while True:
            fired = _wait_for_change(changed, interval)
            if fired:
                changed.clear()
                time.sleep(WATCH_DEBOUNCE_SECONDS)
                changed.clear()
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
    finally:
        if observer is not None:
            try:
                observer.stop()
                observer.join(timeout=2.0)
            except Exception:  # noqa: BLE001 - shutdown must not raise
                pass


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
