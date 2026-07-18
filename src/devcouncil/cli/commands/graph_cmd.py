"""``dev graph`` — query / trace / dead / check / process / impact / html / view / export."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import List, Optional

import typer
from rich.console import Console

app = typer.Typer(
    name="graph",
    help="Query and visualize the symbol-level code knowledge graph.",
    add_completion=False,
)
hooks_app = typer.Typer(name="hooks", help="Optional Git hook integration.", add_completion=False)
app.add_typer(hooks_app, name="hooks")
console = Console()
status = Console(stderr=True)
logger = logging.getLogger(__name__)


def _root(project_root: Path) -> Path:
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    return root


def _graph_degraded_fields(root: Path) -> dict[str, object]:
    """Lean-map handshake for CLI JSON that bypasses the codeintel envelope."""
    map_path = root / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        return {"graph_degraded": False}
    try:
        from devcouncil.utils.json_persist import read_json

        data = read_json(map_path)
        if not isinstance(data, dict):
            return {"graph_degraded": False}
        degraded = bool(data.get("graph_degraded"))
        fields: dict[str, object] = {"graph_degraded": degraded}
        if degraded:
            fields["graph_degraded_reason"] = str(data.get("graph_degraded_reason") or "")
        return fields
    except Exception:
        return {"graph_degraded": False}


def _require_graph(root: Path):
    from devcouncil.indexing.graph.build import load_code_graph

    graph = load_code_graph(root)
    if graph is None:
        status.print("[red]No code graph; run `dev map` first.[/red]")
        raise typer.Exit(code=1)
    return graph


@app.command("init")
def graph_init(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    no_liveness: bool = typer.Option(False, "--no-liveness"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Build the canonical SQLite graph and deterministic compatibility exports."""
    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.codeintel.build_control import GraphBuildBusy
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    root = _root(project_root)
    try:
        refresh = refresh_map_artifacts(
            root,
            root / ".devcouncil" / "repo_map.json",
            liveness=not no_liveness,
            quiet=True,
        )
    except GraphBuildBusy as exc:
        if json_output:
            typer.echo(json.dumps({"ok": False, "code": "graph_writer_busy", "error": str(exc)}))
        else:
            status.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    if refresh.degraded:
        payload = {
            "ok": False,
            "degraded": True,
            "reason": refresh.reason,
            "mode": refresh.mode,
        }
        if json_output:
            typer.echo(json.dumps(payload, indent=2))
        else:
            status.print(f"[red]Graph init degraded: {refresh.reason}[/red]")
        raise typer.Exit(code=1)
    result = get_codeintel_service(root).status()
    if refresh.compatibility_export_degraded:
        result["compatibility_export"] = "degraded"
        result["degraded_reason"] = refresh.reason
    if json_output:
        typer.echo(json.dumps(result, indent=2))
    else:
        color = "yellow" if refresh.compatibility_export_degraded else "green"
        status.print(
            f"[{color}]Indexed generation {result.get('generation')} — "
            f"{result.get('node_count')} nodes, {result.get('edge_count')} edges"
            f"{f'; export degraded: {refresh.reason}' if refresh.compatibility_export_degraded else ''}"
            f"[/{color}]"
        )


@app.command("status")
def graph_status(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Show canonical generation, watcher health, and pending files."""
    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.codeintel.sync import get_sync_coordinator

    root = _root(project_root)
    result = get_codeintel_service(root).status()
    # Cold start: existing compatibility JSON is enough to bootstrap queries/status
    # without requiring a full ``dev map`` rebuild first.
    if result.get("state") in {"uninitialized", "empty"}:
        try:
            from devcouncil.indexing.graph.build import graph_path, load_code_graph

            if graph_path(root).is_file():
                load_code_graph(root)
                result = get_codeintel_service(root).status()
        except Exception:
            logger.debug("graph status cold-start bootstrap failed", exc_info=True)
    result["sync"] = get_sync_coordinator(root).status().as_dict()
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    console.print(f"state: {result['state']}")
    console.print(f"generation: {result.get('generation') or '(none)'}")
    console.print(f"nodes/edges: {result.get('node_count', 0)}/{result.get('edge_count', 0)}")
    sync = result["sync"]
    console.print(f"watcher: {sync['state']} ({sync.get('backend') or 'not started'})")
    if sync.get("state") in {"disabled", "stopped", ""} or not sync.get("backend"):
        console.print(
            "[dim]hint: run `dev graph watch` or `dev map --watch` to enable auto-refresh[/dim]"
        )
    if sync.get("build_id"):
        progress = f"{sync.get('build_completed', 0)}/{sync.get('build_total', 0)}"
        console.print(
            f"build: {sync.get('build_state') or 'unknown'} / "
            f"{sync.get('build_phase') or 'unknown'} ({progress}, "
            f"pid={sync.get('build_pid') or 'n/a'})"
        )
    if sync.get("compatibility_export") == "degraded":
        console.print("compatibility export: degraded")
    if sync.get("pending"):
        console.print("pending: " + ", ".join(sync["pending"]))
    if sync.get("degraded_reason"):
        console.print(f"degraded: {sync['degraded_reason']}")


@app.command("sync")
def graph_sync(
    paths: Optional[List[str]] = typer.Argument(None, help="Optional paths; otherwise reconcile the project."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Reconcile and commit pending filesystem changes now."""
    from devcouncil.codeintel.sync import get_sync_coordinator

    root = _root(project_root)
    coordinator = get_sync_coordinator(root)
    changed = list(paths or coordinator.reconcile())
    ok = coordinator.sync_now(changed)
    result = coordinator.status().as_dict()
    result["ok"] = ok
    result["reconciled"] = changed
    if json_output:
        typer.echo(json.dumps(result, indent=2))
    else:
        color = "green" if ok else "yellow"
        status.print(f"[{color}]Synced {len(changed)} path(s); state={result['state']}[/{color}]")
    if not ok:
        raise typer.Exit(code=1)


@app.command("watch")
def graph_watch(
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Run native auto-sync in the foreground until interrupted."""
    import time

    from devcouncil.codeintel.sync import get_sync_coordinator

    root = _root(project_root)
    coordinator = get_sync_coordinator(root)
    state = coordinator.start()
    status.print(
        f"[cyan]Watching {root} with {state.backend or 'reconciliation'} "
        f"(state={state.state}); Ctrl-C to stop.[/cyan]"
    )
    try:
        while True:
            time.sleep(0.5)
    except KeyboardInterrupt:
        status.print("Stopped watching.")
    finally:
        coordinator.stop()


@app.command("doctor")
def graph_doctor(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Verify SQLite, native watcher selection, and installed grammar assets."""
    from watchdog.observers import Observer

    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.codeintel.build_control import read_build_status
    from devcouncil.codeintel.languages import grammar_status
    from devcouncil.codeintel.store.sqlite import compatibility_graph_digest
    from devcouncil.indexing.graph.build import graph_path
    from devcouncil.utils.json_persist import read_json

    root = _root(project_root)
    service = get_codeintel_service(root)
    store = service.status()
    grammars = grammar_status()
    watcher_backend = getattr(Observer, "__name__", type(Observer).__name__)
    build = read_build_status(root)
    export_path = graph_path(root)
    export_health = "missing"
    export_detail = ""
    if store["state"] == "committed":
        recorded_digest, recorded_mtime = service.store.compatibility_export_state()
        if not export_path.is_file():
            export_health = "missing"
            export_detail = "compatibility JSON absent while store is committed"
        else:
            try:
                data = read_json(export_path)
                from devcouncil.indexing.graph.schema import CodeGraph

                exported = CodeGraph.model_validate(data)
                digest = compatibility_graph_digest(exported)
                if recorded_digest and digest != recorded_digest:
                    export_health = "drift"
                    export_detail = "JSON digest diverges from store handshake"
                elif build.compatibility_export == "degraded":
                    export_health = "degraded"
                    export_detail = build.degraded_reason or "last build skipped JSON export"
                else:
                    export_health = "healthy"
            except Exception as exc:  # noqa: BLE001
                export_health = "corrupt"
                export_detail = f"{type(exc).__name__}: {exc}"
    elif build.compatibility_export == "degraded":
        export_health = "degraded"
        export_detail = build.degraded_reason or "compatibility export degraded"
    result = {
        "ok": store["state"] == "committed" and grammars["ok"] and export_health == "healthy",
        "store": store,
        "watcher_backend": watcher_backend,
        "grammars": grammars,
        "compatibility_export": {
            "health": export_health,
            "detail": export_detail,
            "build_state": build.state,
            "build_compatibility_export": build.compatibility_export,
        },
    }
    # Uninitialized projects are healthy when grammars are installed.
    if store["state"] in {"uninitialized", "empty"}:
        result["ok"] = bool(grammars["ok"])
    if store["state"] == "corrupt":
        result["store_action"] = (
            "index.sqlite is damaged — run `dev map` to quarantine it and rebuild"
        )
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        if not result["ok"]:
            raise typer.Exit(code=1)
        return
    console.print(f"store: {store['state']} (schema {store['schema_version']})")
    if result.get("store_action"):
        console.print(f"store action: {result['store_action']}")
    console.print(f"watcher backend: {watcher_backend}")
    console.print(
        f"compatibility export: {export_health}"
        + (f" — {export_detail}" if export_detail else "")
    )
    console.print(
        f"grammars: {grammars['available_count']}/{grammars['required_count']} available locally"
    )
    for row in grammars["languages"]:
        if not row["available"]:
            # Python parses via stdlib ast regardless of the tree-sitter wheel —
            # don't let a Python-heavy repo read this line as broken indexing.
            native_note = (
                " — extraction unaffected (native stdlib-ast parser)"
                if row.get("grammar") == "python"
                else ""
            )
            console.print(
                f"  missing: {row['language']} "
                f"({', '.join(row['missing_grammars'])}){native_note}"
            )
    if grammars["action"]:
        console.print(f"grammar action: {grammars['action']}")
    if not result["ok"]:
        raise typer.Exit(code=1)


@app.command("search")
def graph_search(
    query: str = typer.Argument(...),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    limit: int = typer.Option(50, "--limit"),
    semantic: bool = typer.Option(False, "--semantic", help="Use opt-in local embeddings when enabled."),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Full-text (or semantic) symbol and path search over the committed generation."""
    root = _root(project_root)
    if semantic:
        from devcouncil.indexing.graph.embeddings import semantic_search

        result = semantic_search(root, query, limit=limit)
        if not result.get("ok"):
            from devcouncil.codeintel.query import CodeIntelQueryEngine

            result = CodeIntelQueryEngine(root).search(query, limit=limit)
    else:
        from devcouncil.codeintel.query import CodeIntelQueryEngine

        result = CodeIntelQueryEngine(root).search(query, limit=limit)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    for match in result.get("matches", []):
        if "line" in match:
            console.print(f"{match['path']}:{match['line']}  {match['id']}  [{match['kind']}]")
        else:
            console.print(f"{match['path']}  {match.get('label', match['id'])}  score={match.get('score')}")


@app.command("ingest")
def graph_ingest(
    paths: Optional[List[str]] = typer.Argument(None, help="Optional paths; full rebuild when omitted."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    no_liveness: bool = typer.Option(False, "--no-liveness"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Unified analyze entry: codeintel sync → graph export → repo map write."""
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts
    from devcouncil.codeintel.sync import get_sync_coordinator
    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.codeintel.build_control import GraphBuildBusy
    from devcouncil.indexing.graph.embeddings import build_embeddings

    root = _root(project_root)
    coordinator = get_sync_coordinator(root)
    changed = list(paths or [])
    map_path = root / ".devcouncil" / "repo_map.json"
    if paths is None:
        try:
            refresh = refresh_map_artifacts(
                root,
                map_path,
                liveness=not no_liveness,
                quiet=True,
            )
        except GraphBuildBusy as exc:
            payload = {
                "ok": False,
                "code": "graph_writer_busy",
                "error": str(exc),
                "paths": changed,
            }
            if json_output:
                typer.echo(json.dumps(payload, indent=2))
            else:
                status.print(f"[red]{exc}[/red]")
            raise typer.Exit(code=1) from exc
    else:
        synced = coordinator.sync_now(changed)
        if not synced:
            payload = {"ok": False, "paths": changed, **coordinator.status().as_dict()}
            if json_output:
                typer.echo(json.dumps(payload, indent=2))
            else:
                status.print(f"[red]Graph ingest failed: {payload.get('last_error') or payload.get('degraded_reason')}[/red]")
            raise typer.Exit(code=1)
        refresh = refresh_map_artifacts(
            root,
            map_path,
            liveness=not no_liveness,
            quiet=True,
            graph=get_codeintel_service(root).load(),
            paths=changed,
        )
    embedded = build_embeddings(root)
    payload = {
        "ok": not refresh.degraded,
        "paths": changed,
        "map": str(map_path.relative_to(root)),
        "embeddings_built": embedded,
        "generation": refresh.generation,
        "mode": refresh.mode,
        "degraded": refresh.degraded,
        "reason": refresh.reason,
    }
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
    else:
        color = "yellow" if refresh.degraded else "green"
        status.print(
            f"[{color}]Ingested {len(changed)} path(s); map at {payload['map']}"
            f"{f'; {embedded} embeddings' if embedded else ''}"
            f"{f'; degraded: {refresh.reason}' if refresh.degraded else ''}[/{color}]"
        )
    if refresh.degraded:
        raise typer.Exit(code=1)


@app.command("cypher")
def graph_cypher(
    query: str = typer.Argument(..., help="Supported MATCH … RETURN subset."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Run a supported Cypher subset over the native SQLite graph store."""
    from devcouncil.indexing.graph.cypher import run_cypher

    result = run_cypher(_root(project_root), query)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        if not result.get("ok"):
            raise typer.Exit(code=1)
        return
    if not result.get("ok"):
        status.print(f"[red]{result.get('error', 'cypher failed')}[/red]")
        raise typer.Exit(code=1)
    for row in result.get("rows", []):
        console.print(" ".join(f"{k}={v}" for k, v in row.items()))


@app.command("explore")
def graph_explore(
    query: str = typer.Argument(...),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    limit: int = typer.Option(20, "--limit"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Return source, related symbols, paths, and blast radius in one query."""
    from devcouncil.codeintel.query import CodeIntelQueryEngine

    result = CodeIntelQueryEngine(_root(project_root)).explore(query, limit=limit)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    for definition in result["definitions"]:
        console.print(f"[bold]{definition['id']}[/bold] {definition['path']}:{definition['line']}")
        if definition["source"]:
            console.print(definition["source"])
        console.print(
            f"  callers={len(definition['callers'])} callees={len(definition['callees'])}"
        )


@app.command("affected")
def graph_affected(
    targets: List[str] = typer.Argument(..., help="Symbol or path targets."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Find tests reachable through the inbound blast radius."""
    from devcouncil.codeintel.query import CodeIntelQueryEngine

    result = CodeIntelQueryEngine(_root(project_root)).affected_tests(targets)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if not result["tests"]:
        console.print("No affected tests found.")
        return
    for test in result["tests"]:
        console.print(test)


@hooks_app.command("install")
def graph_hooks_install(
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Install an opt-in post-checkout/post-merge reconciliation hook."""
    root = _root(project_root)
    git_dir = root / ".git"
    if not git_dir.is_dir():
        status.print("[red]Git hook installation requires a normal .git directory.[/red]")
        raise typer.Exit(code=1)
    hook_body = "#!/bin/sh\nexec dev graph sync --project-root \"$(git rev-parse --show-toplevel)\" >/dev/null 2>&1\n"
    for name in ("post-checkout", "post-merge"):
        path = git_dir / "hooks" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists() and "dev graph sync" not in path.read_text(encoding="utf-8", errors="replace"):
            status.print(f"[red]Refusing to overwrite existing hook: {path}[/red]")
            raise typer.Exit(code=1)
        path.write_text(hook_body, encoding="utf-8")
        path.chmod(0o755)
    status.print("[green]Installed post-checkout and post-merge code-intelligence hooks.[/green]")


@app.command("query")
def graph_query(
    name_or_path: str = typer.Argument(..., help="Symbol name or file path."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """360° view: definition, callers, callees, importers."""
    from devcouncil.indexing.graph import query_symbol

    root = _root(project_root)
    result = {**query_symbol(root, name_or_path), **_graph_degraded_fields(root)}
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if result.get("graph_degraded"):
        status.print(
            f"[yellow]graph_degraded: {result.get('graph_degraded_reason') or 'lean map'}[/yellow]"
        )
    if result.get("error"):
        status.print(f"[red]{result['error']}[/red]")
        raise typer.Exit(code=1)
    defs = result.get("definitions") or []
    if not defs:
        console.print(f"No matches for {name_or_path!r}")
        return
    for d in defs:
        console.print(f"[bold]{d['id']}[/bold]  ({d.get('kind')})  {d.get('path')}:{d.get('line')}")
        console.print(f"  callers: {', '.join(d.get('callers') or []) or '(none)'}")
        console.print(f"  callees: {', '.join(d.get('callees') or []) or '(none)'}")
        console.print(f"  importers: {', '.join(d.get('importers') or []) or '(none)'}")


@app.command("trace")
def graph_trace(
    start: str = typer.Argument(..., help="Start node (name or path)."),
    end: str = typer.Argument(..., help="End node (name or path)."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Shortest path between two graph nodes."""
    from devcouncil.indexing.graph import trace_path

    root = _root(project_root)
    result = {**trace_path(root, start, end), **_graph_degraded_fields(root)}
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if result.get("graph_degraded"):
        status.print(
            f"[yellow]graph_degraded: {result.get('graph_degraded_reason') or 'lean map'}[/yellow]"
        )
    if result.get("error"):
        status.print(f"[red]{result['error']}[/red]")
        raise typer.Exit(code=1)
    if not result.get("found"):
        console.print(f"No path between {start!r} and {end!r}")
        raise typer.Exit(code=1)
    console.print(" → ".join(result.get("path") or []))


@app.command("dead")
def graph_dead(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    confidence: Optional[str] = typer.Option(
        None, "--confidence", help="Exact filter: extracted|inferred|ambiguous"
    ),
    min_confidence: str = typer.Option(
        "inferred",
        "--min-confidence",
        help="Include this tier and above: extracted > inferred > ambiguous "
        "(default: inferred; pass ambiguous to show all)",
    ),
) -> None:
    """Full dead-code report with confidence tiers and reasons."""
    from collections import Counter

    from devcouncil.indexing.graph.liveness import confidence_at_least

    root = _root(project_root)
    graph = _require_graph(root)
    entries = list(graph.dead_code)
    if confidence:
        entries = [
            e
            for e in entries
            if (e.confidence.value if hasattr(e.confidence, "value") else str(e.confidence))
            == confidence
        ]
    before_min = len(entries)
    if min_confidence:
        entries = [
            e
            for e in entries
            if confidence_at_least(e.confidence, min_confidence)
        ]
    hidden = before_min - len(entries)
    degraded = _graph_degraded_fields(root)
    if json_output:
        typer.echo(
            json.dumps(
                {
                    "dead_code": [e.model_dump() for e in entries],
                    "dead_code_hidden": hidden,
                    **degraded,
                },
                indent=2,
            )
        )
        return
    if degraded.get("graph_degraded"):
        status.print(
            f"[yellow]graph_degraded: {degraded.get('graph_degraded_reason') or 'lean map'} "
            "— treat dead tiers as unreliable[/yellow]"
        )
    if not entries:
        console.print("No dead-code entries.")
        if hidden:
            console.print(
                f"{hidden} lower-confidence entries hidden "
                "(--min-confidence ambiguous to show)."
            )
        return
    for e in entries:
        conf = e.confidence.value if hasattr(e.confidence, "value") else e.confidence
        console.print(
            f"{e.path}:{e.line}  {e.id}  [{conf}/{e.kind}]  {e.reason}"
        )
    reason_counts = Counter(e.reason or "(none)" for e in entries)
    console.print("")
    console.print("Reason summary:")
    for reason, n in reason_counts.most_common():
        console.print(f"  {n:4d}  {reason}")
    if hidden:
        console.print("")
        console.print(
            f"{hidden} lower-confidence entries hidden "
            "(--min-confidence ambiguous to show)."
        )


@app.command("check")
def graph_check_cmd(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    top: int = typer.Option(15, "--top", help="How many god nodes to list."),
) -> None:
    """God nodes (top-connected) and circular-import component detection."""
    from devcouncil.indexing.graph.intel import graph_check

    root = _root(project_root)
    graph = _require_graph(root)
    report = graph_check(graph, top_n=top)
    if json_output:
        typer.echo(json.dumps(report, indent=2))
        return
    console.print(f"[bold]God nodes[/bold] (top {top} by degree)")
    for g in report.get("god_nodes") or []:
        console.print(
            f"  {g.get('degree'):>4}  {g.get('id')}  ({g.get('kind')})"
        )
    cycles = report.get("circular_imports") or []
    console.print(
        f"\n[bold]Circular imports — strongly connected components[/bold] ({len(cycles)})"
    )
    if not cycles:
        console.print("  (none)")
    for c in cycles[:30]:
        console.print("  " + " ↔ ".join(c.get("nodes") or []))
    package_init_count = report.get("package_init_count", 0)
    if package_init_count:
        console.print(
            f"  {package_init_count} package-__init__ component(s) suppressed as barrel noise"
        )


@app.command("process")
def graph_process(
    entry: Optional[str] = typer.Argument(
        None, help="Optional entry root path or name filter."
    ),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    max_depth: int = typer.Option(6, "--max-depth"),
) -> None:
    """BFS call-flows from entry roots (named, step-ordered, depth-capped)."""
    from devcouncil.indexing.graph.intel import extract_processes

    root = _root(project_root)
    graph = _require_graph(root)
    processes = extract_processes(graph, entry=entry, max_depth=max_depth)
    if json_output:
        typer.echo(json.dumps(processes, indent=2))
        return
    if not processes:
        console.print("No processes found.")
        return
    for p in processes:
        console.print(f"[bold]{p.get('name')}[/bold]  (depth {p.get('depth')})")
        console.print("  " + " → ".join(p.get("steps") or []))


@app.command("impact")
def graph_impact(
    paths: Optional[List[str]] = typer.Argument(
        None, help="Paths to analyze (omit with --diff for working-tree changes)."
    ),
    diff: bool = typer.Option(
        False, "--diff", help="Use working-tree changed files as the seed set."
    ),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    max_depth: int = typer.Option(3, "--max-depth", help="Inbound blast depth (1–3)."),
) -> None:
    """Diff / path blast radius via enclosing symbols and inbound callers."""
    from devcouncil.indexing.graph.intel import diff_impact

    root = _root(project_root)
    graph = _require_graph(root)
    if not diff and not paths:
        status.print("[red]Provide paths or --diff.[/red]")
        raise typer.Exit(code=1)
    result = diff_impact(
        root,
        graph,
        paths=paths,
        use_diff=diff,
        max_depth=max(1, min(3, max_depth)),
    )
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if not result.get("paths"):
        console.print("No impacted paths.")
        return
    for item in result["paths"]:
        console.print(f"[bold]{item['path']}[/bold]")
        syms = item.get("symbols") or []
        if syms:
            console.print("  symbols: " + ", ".join(s["id"] for s in syms[:8]))
        for layer in (item.get("blast") or {}).get("layers") or []:
            nodes = layer.get("nodes") or []
            console.print(
                f"  depth {layer['depth']} [{layer['confidence']}]: "
                f"{len(nodes)} — " + ", ".join(nodes[:6])
                + (" …" if len(nodes) > 6 else "")
            )


@app.command("html")
def graph_html(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    open_browser: bool = typer.Option(False, "--open", help="Open in the default browser."),
    symbols: bool = typer.Option(
        False,
        "--symbols",
        help="Default the visualizer to symbol-level mode (calls/inherits) instead of file imports.",
    ),
) -> None:
    """Write a self-contained interactive ``graph.html``."""
    from devcouncil.indexing.viz import write_graph_html

    root = _root(project_root)
    try:
        out = write_graph_html(root, open_browser=open_browser, symbols=symbols)
    except FileNotFoundError as exc:
        status.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    status.print(f"[green]Wrote {out}[/green]")


@app.command("demo")
def graph_demo(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    open_browser: bool = typer.Option(False, "--open", help="Open the interactive demo."),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Write sample graph HTML and SVG artifacts without requiring a repo map."""
    from devcouncil.indexing.viz import write_graph_demo

    paths = write_graph_demo(_root(project_root), open_browser=open_browser)
    payload = {name: str(path) for name, path in paths.items()}
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
        return
    status.print(f"[green]Wrote {payload['html']} and {payload['svg']}[/green]")


@app.command("view")
def graph_view(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    port: int = typer.Option(8765, "--port"),
) -> None:
    """Serve/open the graph HTML via a tiny local HTTP server."""
    import http.server
    import socketserver
    import threading
    import webbrowser

    from devcouncil.indexing.viz import write_graph_html

    root = _root(project_root)
    try:
        out = write_graph_html(root, open_browser=False)
    except FileNotFoundError as exc:
        status.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc

    directory = str(out.parent)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=directory, **kwargs)

        def log_message(self, fmt, *args):  # noqa: A003
            return

    try:
        httpd = socketserver.TCPServer(("127.0.0.1", port), Handler)
    except OSError as exc:
        status.print(f"[red]Cannot serve on 127.0.0.1:{port}: {exc} (try --port)[/red]")
        raise typer.Exit(code=1) from exc
    with httpd:
        url = f"http://127.0.0.1:{port}/graph.html"
        status.print(f"[green]Serving {url}  (Ctrl-C to stop)[/green]")
        threading.Timer(0.3, lambda: webbrowser.open(url)).start()
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            status.print("Stopped.")


@app.command("export")
def graph_export(
    format: str = typer.Option(
        "graphml",
        "--format",
        help="graphml | okf | okf-links",
    ),
    output: Path = typer.Option(Path("-"), "--output", "-o"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Export the code graph as attributed GraphML or an OKF v0.1 bundle."""
    from devcouncil.indexing.graph.export import export_graphml, write_code_graph_okf

    root = _root(project_root)
    graph = _require_graph(root)
    fmt = format.lower().strip()
    if fmt == "graphml":
        text = export_graphml(graph)
        if str(output) == "-":
            typer.echo(text)
        else:
            out = output if output.is_absolute() else root / output
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(text, encoding="utf-8")
            status.print(f"[green]Wrote {out}[/green]")
        return
    if fmt == "okf":
        if str(output) == "-":
            status.print("[red]OKF export requires -o <directory>[/red]")
            raise typer.Exit(code=1)
        out_dir = output if output.is_absolute() else root / output
        try:
            written_dir, paths = write_code_graph_okf(root, out_dir, graph=graph)
        except FileNotFoundError as exc:
            status.print(f"[red]{exc}[/red]")
            raise typer.Exit(code=1) from exc
        status.print(f"[green]Wrote OKF bundle ({len(paths)} docs) to {written_dir}[/green]")
        return
    if fmt in {"okf-links"}:
        rows = []
        for e in graph.edges:
            if e.kind in {"imports", "calls"}:
                rows.append(f"{e.source} --{e.kind}--> {e.target}")
        text = "\n".join(rows)
        if str(output) == "-":
            typer.echo(text)
        else:
            out = output if output.is_absolute() else root / output
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(text, encoding="utf-8")
            status.print(f"[green]Wrote {out}[/green]")
        return
    status.print(f"[red]Unknown format: {format}[/red]")
    raise typer.Exit(code=1)


@app.command("routes")
def graph_routes(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Map HTTP routes to handlers and client fetch consumers."""
    from devcouncil.indexing.graph.api_routes import route_map

    root = _root(project_root)
    graph = _require_graph(root)
    result = route_map(root, graph)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    routes = result.get("routes") or []
    if not routes:
        console.print("No routes found.")
        return
    for route in routes:
        console.print(
            f"[bold]{route.get('verb')} {route.get('path')}[/bold] "
            f"({route.get('framework') or 'unknown'})"
        )
        handlers = route.get("handlers") or []
        if handlers:
            console.print("  handlers: " + ", ".join(h.get("id", "?") for h in handlers[:4]))
        consumers = route.get("consumers") or []
        if consumers:
            console.print(f"  consumers: {len(consumers)}")


@app.command("shape-check")
def graph_shape_check(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    route: Optional[str] = typer.Option(None, "--route", help="Filter to one route path or id."),
) -> None:
    """Compare handler response keys vs client accessed keys."""
    from devcouncil.indexing.graph.api_routes import shape_check

    root = _root(project_root)
    graph = _require_graph(root)
    result = shape_check(root, graph, route_filter=route)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    mismatches = result.get("mismatches") or []
    if not mismatches:
        console.print("[green]No shape mismatches.[/green]")
        return
    for item in mismatches:
        console.print(
            f"[yellow]{item.get('verb')} {item.get('route')}[/yellow] — "
            f"missing in handler: {', '.join(item.get('missing_in_handler') or [])}"
        )


@app.command("api-impact")
def graph_api_impact(
    route_or_path: str = typer.Argument(..., help="Route path, id, or normalized segment."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """API blast radius: consumers, middleware, shape mismatches, risk tier."""
    from devcouncil.indexing.graph.api_routes import api_impact

    root = _root(project_root)
    graph = _require_graph(root)
    result = api_impact(root, route_or_path, graph)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if not result.get("found"):
        console.print(f"[red]Route not found:[/red] {route_or_path}")
        raise typer.Exit(code=1)
    console.print(
        f"[bold]{result.get('verb')} {result.get('route')}[/bold] "
        f"risk={result.get('risk')}"
    )
    console.print(f"  consumers: {len(result.get('consumers') or [])}")
    console.print(f"  middleware: {len(result.get('middleware') or [])}")
    mismatches = result.get("shape_mismatches") or []
    if mismatches:
        console.print(f"  shape mismatches: {len(mismatches)}")


corpus_app = typer.Typer(
    name="corpus",
    help="Advisory mixed-corpus index for docs, PDFs, and images (never verify gates).",
    add_completion=False,
)


@corpus_app.command("build")
def corpus_build(
    path: Optional[str] = typer.Option(None, "--path", help="Root file or directory to index."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Build ``.devcouncil/corpus/graph.json`` from docs, PDFs, and images."""
    from devcouncil.indexing.wiring import build_corpus, corpus_status

    root = _root(project_root)
    build_corpus(root, path=path)
    result = corpus_status(root)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    status.print(
        f"[green]Corpus indexed — {result['node_count']} nodes, "
        f"{result['edge_count']} edges → {result.get('graph_path')}[/green]"
    )


@corpus_app.command("query")
def corpus_query(
    query: str = typer.Argument(..., help="Search string."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    limit: int = typer.Option(20, "--limit"),
) -> None:
    """Search the advisory corpus graph."""
    from devcouncil.indexing.wiring import query_corpus

    root = _root(project_root)
    result = query_corpus(root, query, limit=limit)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        if result.get("error"):
            raise typer.Exit(code=1)
        return
    if result.get("error"):
        status.print(f"[red]{result['error']}[/red]")
        raise typer.Exit(code=1)
    matches = result.get("matches") or []
    if not matches:
        console.print(f"No matches for {query!r}")
        return
    for item in matches:
        console.print(
            f"[bold]{item['label']}[/bold]  ({item['kind']})  "
            f"{item.get('path') or ''}  score={item.get('score')}"
        )


@corpus_app.command("status")
def corpus_status_cmd(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Show corpus artifact freshness and counts."""
    from devcouncil.indexing.wiring import corpus_status

    root = _root(project_root)
    result = corpus_status(root)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    console.print(f"enabled: {result['enabled']}")
    console.print(f"graph: {result.get('graph_path') or '(not built)'}")
    console.print(f"built_at: {result.get('built_at') or '(none)'}")
    console.print(f"nodes/edges: {result.get('node_count', 0)}/{result.get('edge_count', 0)}")
    console.print("advisory: yes (does not feed verify gates)")


pdg_app = typer.Typer(
    name="pdg",
    help="Opt-in CFG / reaching-def / CDG / taint analysis (Python, intra-procedural).",
    add_completion=False,
)
app.add_typer(pdg_app, name="pdg")


@pdg_app.command("build")
def graph_pdg_build(
    paths: List[str] = typer.Option([], "--path", help="Limit analysis to these repo-relative files."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Build or refresh the PDG layer for Python files."""
    from devcouncil.indexing.graph.build import (
        build_pdg_for_paths,
        merge_pdg_into_graph,
        write_code_graph,
    )

    root = _root(project_root)
    graph = _require_graph(root)
    layer = build_pdg_for_paths(root, graph, paths=paths or None)
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
    payload = {"ok": True, "stats": stats, "files": sorted(layer.files.keys())}
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
        return
    console.print(
        f"PDG: {stats.get('function_count', 0)} functions, "
        f"{stats.get('taint_count', 0)} taint findings across {stats.get('file_count', 0)} files"
    )


@app.command("explain")
def graph_explain(
    path: Optional[str] = typer.Option(None, "--path", help="Filter by file path."),
    category: Optional[str] = typer.Option(None, "--category", help="Filter by taint category."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Report heuristic taint findings from the opt-in PDG layer."""
    from devcouncil.indexing.graph.query import explain_pdg_taint

    root = _root(project_root)
    result = explain_pdg_taint(root, path=path, category=category)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        if not result.get("ok"):
            raise typer.Exit(code=1)
        return
    if not result.get("ok"):
        status.print(f"[red]{result.get('error')}[/red]")
        raise typer.Exit(code=1)
    findings = result.get("findings") or []
    if not findings:
        console.print("No taint findings.")
        return
    for item in findings:
        console.print(
            f"{item.get('path')}:{item.get('sink_line')}  "
            f"[{item.get('category')}]  {item.get('function')}  "
            f"{item.get('source_expr')} -> {item.get('sink_expr')}"
        )


@app.command("pdg-query")
def graph_pdg_query(
    mode: str = typer.Option(..., "--mode", help="controls or flows"),
    target: str = typer.Option(..., "--target", help="Symbol qualname or file path."),
    variable: Optional[str] = typer.Option(None, "--variable", help="Filter flows by variable."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Query control or data dependence for an anchored target."""
    from devcouncil.indexing.graph.query import query_pdg_controls, query_pdg_flows

    root = _root(project_root)
    if mode == "controls":
        result = query_pdg_controls(root, target)
    elif mode == "flows":
        result = query_pdg_flows(root, target, variable=variable)
    else:
        status.print("[red]--mode must be controls or flows[/red]")
        raise typer.Exit(code=2)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        if not result.get("ok"):
            raise typer.Exit(code=1)
        return
    if not result.get("ok"):
        status.print(f"[red]{result.get('error')}[/red]")
        raise typer.Exit(code=1)
    for fn in result.get("functions") or []:
        console.print(f"[bold]{fn.get('qualname')}[/bold]  ({fn.get('path')})")
        key = "cdg" if mode == "controls" else "reaching_def"
        for edge in fn.get(key) or []:
            console.print(f"  {edge}")
