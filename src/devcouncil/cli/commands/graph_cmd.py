"""Symbol-graph CLI commands (mounted under ``dev map``; ``dev graph`` is an alias)."""

from __future__ import annotations

import json
import logging
import time
from pathlib import Path
from typing import List, Optional

import typer
from rich.console import Console

app = typer.Typer(
    name="graph",
    help=(
        "Query and visualize the symbol-level code knowledge graph "
        "(prefer `dev map …`; `dev graph` remains a compatibility alias)."
    ),
    add_completion=False,
)
hooks_app = typer.Typer(
    name="hooks",
    help="Optional Git hook integration (prefer `dev map hooks install`).",
    add_completion=False,
)
app.add_typer(hooks_app, name="hooks")
console = Console()
status = Console(stderr=True)
logger = logging.getLogger(__name__)

# Shared with `dev map --watch` / `dev map watch` / alias `dev graph watch`.
WATCH_DEBOUNCE_SECONDS = 0.8


def run_foreground_watch(
    root: Path,
    *,
    liveness: bool = True,
    out: Console | None = None,
) -> None:
    """Run the shared code-intelligence coordinator until interrupted.

    All watch entry points must use the same coordinator kwargs so a second
    call does not raise ``ValueError`` on mismatched debounce/callback.
    """
    from devcouncil.codeintel.sync import get_sync_coordinator
    from devcouncil.indexing.graph.build import refresh_map_for_paths

    printer = out or status
    coordinator = get_sync_coordinator(
        root,
        debounce_seconds=WATCH_DEBOUNCE_SECONDS,
        sync_callback=lambda paths: refresh_map_for_paths(root, paths, liveness=liveness),
    )
    state = coordinator.start()
    printer.print(
        f"[cyan]Watching {root} with {state.backend or 'reconciliation'} "
        f"(state={state.state}, debounce {WATCH_DEBOUNCE_SECONDS}s). Ctrl-C to stop.[/cyan]"
    )
    try:
        while True:
            time.sleep(WATCH_DEBOUNCE_SECONDS)
            before = coordinator.status().pending
            if before and not coordinator.sync_now():
                failure = coordinator.status().last_error or coordinator.status().degraded_reason
                printer.print(f"[yellow]Watch refresh failed (ignored): {failure}[/yellow]")
            elif before:
                printer.print(f"[green]Refreshed map for {len(before)} path(s)[/green]")
    except KeyboardInterrupt:
        printer.print("Stopped watching.")
    finally:
        coordinator.stop(timeout=2)


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


def _canonical_store_health(root: Path) -> str:
    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.indexing.graph.communities import store_health_from_state

    state = get_codeintel_service(root).status()
    return store_health_from_state(str(state.get("state") or ""))


def _emit_limit(out: Console, limit_dict: dict) -> None:
    if not limit_dict.get("degraded"):
        return
    out.print(f"[yellow]limit ({limit_dict.get('kind')}): {limit_dict.get('reason')}[/yellow]")
    if limit_dict.get("recovery_command"):
        out.print(f"[dim]recovery: {limit_dict['recovery_command']}[/dim]")
    if limit_dict.get("detail"):
        out.print(f"[dim]{limit_dict['detail']}[/dim]")


def _index_freshness_fields(root: Path) -> dict[str, object]:
    """Freshness probe for read commands; never raises."""
    try:
        from devcouncil.codeintel.service import index_freshness

        return index_freshness(root)
    except Exception as exc:  # noqa: BLE001 - probe must not break reads
        return {"fresh": None, "reason": f"freshness probe failed: {exc}"}



#: Edge kinds that represent one symbol invoking another. The devmap store also
#: emits structural edges (`Contains` for file→symbol, `MemberOf` for
#: symbol→type); including those in a caller/callee list makes a symbol look like
#: it calls itself and inflates blast radius with edges nobody can act on.
_CALL_EDGE_KINDS = frozenset({"Calls"})


def _call_edges(client, method: str, target: str, symbol_key: str, file_key: str):
    """Return ``(edges, unavailable_reason)`` for one direction of the call graph.

    Fail-closed, deliberately. The previous form was::

        try:
            resp = client.impact(...)
            if resolution_unavailable_reason(resp.resolution) is None:
                ...collect...
        except DevMapClientError:
            pass

    which produced an empty list on *three* different outcomes: a genuine absence
    of callers, a transport error, and a store that reported its resolution
    unavailable. A consumer asking "what calls this before I delete it" could not
    tell those apart, and two of them are the answer "I don't know."

    That is the exact conflation :func:`devcouncil.devmap_client.try_connect`
    exists to prevent — *"a check that could not run must never report what a
    check that ran and passed reports."* This applies the same rule one layer up:
    on failure the caller gets ``(None, reason)``, and ``None`` is not ``[]``.
    """
    from devcouncil.devmap_client import (
        DevMapClientError,
        resolution_unavailable_reason,
    )

    try:
        resp = getattr(client, method)(target, depth=1)
    except DevMapClientError as exc:
        return None, f"{method} failed: {exc}"

    reason = resolution_unavailable_reason(resp.resolution)
    if reason:
        return None, f"{method} resolution unavailable: {reason}"

    edges = []
    for edge in resp.items:
        if str(edge.get("edge_kind") or "") not in _CALL_EDGE_KINDS:
            continue
        node = str(edge.get(symbol_key) or edge.get(file_key) or "")
        if node:
            edges.append(node)
    return edges, None


def _render_edge_field(definition: dict, field: str) -> str:
    """Render one edge list, keeping "empty" and "unknown" visibly different.

    The old renderer printed ``(none)`` for both, so a transport failure looked
    exactly like a symbol nothing calls — which is the reading that gets a live
    function deleted.
    """
    value = definition.get(field)
    if value is None:
        reason = definition.get(f"{field}_unavailable") or "not measured"
        return f"[yellow](unknown — {reason})[/yellow]"
    return ", ".join(value) or "(none)"


def _devmap_query_payload(root: Path, kind: str, **kwargs):
    """Try DevMapClient for query surfaces; return payload or None for Python fallback."""
    from devcouncil.devmap_client import (
        DevMapClientError,
        resolution_unavailable_reason,
        try_connect,
    )

    client = try_connect(root)
    if client is None:
        return None
    try:
        if kind == "status":
            st = client.status()
            return {
                "state": "committed" if st.generation_id > 0 else "empty",
                "generation": st.generation_id or None,
                "node_count": st.node_count,
                "edge_count": st.edge_count,
                "pending_count": st.pending_count,
                "is_fresh": st.is_fresh,
                "degraded_reason": st.degraded_reason,
                "source": "devmap",
                "index_freshness": {
                    "fresh": st.is_fresh,
                    "generation": st.generation_id,
                    "reason": st.degraded_reason,
                },
                "sync": {
                    "state": "fresh" if st.is_fresh and st.pending_count == 0 else "pending",
                    "pending": [],
                    "fresh": st.is_fresh,
                    "backend": "devmap",
                    "degraded_reason": st.degraded_reason,
                },
            }
        if kind == "search":
            query = str(kwargs["query"])
            limit = int(kwargs.get("limit", 50))
            resp = client.search(query, limit=2000, semantic=bool(kwargs.get("semantic")))
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                raise DevMapClientError(reason)
            if resp.total > 0 and not resp.items and resp.truncated:
                raise DevMapClientError("truncated empty search")
            matches = []
            for item in resp.items[:limit]:
                path_s = str(item.get("file_path") or "")
                name = str(item.get("symbol_name") or "")
                span = item.get("span") or (0, 0)
                line = int(span[0]) if isinstance(span, (list, tuple)) and span else 0
                matches.append({
                    "id": f"{path_s}::{name}" if path_s and name else name or path_s,
                    "path": path_s,
                    "name": name,
                    "kind": str(item.get("kind") or "symbol"),
                    "line": line,
                    "score": item.get("score"),
                })
            return {
                "ok": True,
                "query": query,
                "matches": matches,
                "source": "devmap",
                "truncated": resp.truncated or len(resp.items) > limit,
                "total": resp.total,
                **_graph_degraded_fields(root),
            }
        if kind == "trace":
            start = str(kwargs["start"])
            end = str(kwargs["end"])
            resp = client.trace(start, depth=3, to_symbol=end)
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                return {
                    "ok": True,
                    "found": False,
                    "error": reason,
                    "path": [],
                    "source": "devmap",
                    **_graph_degraded_fields(root),
                }
            nodes = []
            for item in resp.items:
                node = str(
                    item.get("node")
                    or item.get("symbol_name")
                    or item.get("target_symbol")
                    or item.get("source_symbol")
                    or ""
                )
                if node:
                    nodes.append(node)
            return {
                "ok": True,
                "found": bool(nodes),
                "path": nodes,
                "source": "devmap",
                "truncated": resp.truncated,
                **_graph_degraded_fields(root),
            }
        if kind == "query":
            name_or_path = str(kwargs["name_or_path"])
            resp = client.search(name_or_path, limit=2000)
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                raise DevMapClientError(reason)
            if resp.total > 0 and not resp.items and resp.truncated:
                raise DevMapClientError("truncated empty query")
            defs = []
            for item in resp.items[:20]:
                path_s = str(item.get("file_path") or "")
                name = str(item.get("symbol_name") or "")
                span = item.get("span") or (0, 0)
                line = int(span[0]) if isinstance(span, (list, tuple)) and span else 0
                node_id = f"{path_s}::{name}" if path_s and name else name or path_s
                target = node_id if name else path_s or name_or_path
                callers, callers_unavailable = _call_edges(
                    client, "impact", target, "source_symbol", "source_file"
                )
                # Symbol-scoped, like the inbound side. This used to pass
                # `path_s` — the FILE — so a function's "callees" were the whole
                # file's outbound edges. `IsRestatement` reported 35 callees
                # where the symbol-scoped answer is 0, and the list included
                # `Contains`/`MemberOf` structural edges, which is why symbols
                # appeared to call themselves.
                callees, callees_unavailable = _call_edges(
                    client, "deps", target, "target_symbol", "target_file"
                )
                defs.append({
                    "id": node_id,
                    "kind": str(item.get("kind") or "symbol"),
                    "path": path_s,
                    "name": name,
                    "line": line,
                    "callers": callers,
                    "callers_unavailable": callers_unavailable,
                    "callees": callees,
                    "callees_unavailable": callees_unavailable,
                    # Not computed. The devmap store surfaces Calls/Contains/
                    # MemberOf edges here, not Imports, so there is nothing to
                    # derive an importer list from. This was previously a
                    # hardcoded `[]`, which reads as "nothing imports this"
                    # rather than "never measured" — the same conflation
                    # `try_connect` refuses to make about an empty store.
                    "importers": None,
                    "importers_unavailable": "not computed: devmap exposes no Imports edges",
                })
            return {
                "ok": True,
                "definitions": defs,
                "source": "devmap",
                **_graph_degraded_fields(root),
            }
        if kind == "dead":
            resp = client.dead_symbols(budget=2000)
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                raise DevMapClientError(reason)
            entries = []
            for item in resp.items:
                path_s = str(item.get("file_path") or item.get("path") or "")
                name = str(item.get("symbol_name") or item.get("name") or "")
                span = item.get("span") or (0, 0)
                line = int(span[0]) if isinstance(span, (list, tuple)) and span else int(item.get("line") or 0)
                conf = item.get("confidence", "inferred")
                if isinstance(conf, (int, float)):
                    score = float(conf)
                    conf = "extracted" if score >= 0.9 else "inferred" if score >= 0.5 else "ambiguous"
                entries.append({
                    "id": str(item.get("id") or (f"{path_s}::{name}" if path_s and name else name or path_s)),
                    "path": path_s,
                    "name": name,
                    "line": line,
                    "confidence": str(conf),
                    "kind": str(item.get("kind") or "symbol"),
                    "reason": str(item.get("reason") or item.get("details") or ""),
                })
            return {
                "dead_code": entries,
                "dead_code_hidden": resp.hidden,
                "source": "devmap",
                "truncated": resp.truncated,
                "total": resp.total,
                **_graph_degraded_fields(root),
            }
        if kind == "impact":
            paths = list(kwargs.get("paths") or [])
            depth = int(kwargs.get("max_depth", 3))
            items = []
            for path_s in paths:
                resp = client.impact(path_s, depth=max(1, min(3, depth)))
                reason = resolution_unavailable_reason(resp.resolution)
                if reason:
                    raise DevMapClientError(f"{path_s}: {reason}")
                nodes = sorted({
                    str(edge.get("source_symbol") or edge.get("source_file") or "")
                    for edge in resp.items
                    if edge.get("source_symbol") or edge.get("source_file")
                })
                items.append({
                    "path": path_s,
                    "symbols": [],
                    "blast": {
                        "layers": [{
                            "depth": 1,
                            "nodes": nodes,
                            "confidence": "extracted",
                            "count": len(nodes),
                        }] if nodes else [],
                        "total_impacted": len(nodes),
                    },
                    "resolution": "devmap",
                })
            return {"ok": True, "paths": items, "source": "devmap", **_graph_degraded_fields(root)}
    except DevMapClientError as exc:
        logger.warning(
                "devmap (Rust) %s failed and this call fell back to the Python path: %s. "
                "The Rust kernel is primary; a fallback here is a defect, not a mode.",
                kind,
                exc,
            )
        return None
    return None


def _warn_if_stale(
    root: Path, *, note_unknown: bool = False, quiet: bool = False
) -> dict[str, object]:
    """Print a loud staleness banner on stderr; return the freshness fields.

    A frozen index must never pass silently as current evidence — results from
    an index built at another commit misled a deletion decision on 2026-08-11.
    ``quiet`` suppresses banners for JSON mode, where the structured
    ``index_freshness`` field (plus the exit code) is the machine signal.
    """
    fields = _index_freshness_fields(root)
    if quiet:
        return fields
    if fields.get("fresh") is False:
        age = fields.get("age_seconds")
        age_text = f" ({age / 3600.0:.1f}h old)" if isinstance(age, (int, float)) else ""
        status.print(
            f"[red]index STALE: {fields.get('reason') or 'index head does not match HEAD'}"
            f"{age_text} — results reflect the old commit; run `dev map` to refresh[/red]"
        )
    elif note_unknown and fields.get("fresh") is None and fields.get("generation") is not None:
        status.print(
            f"[yellow]index freshness unknown: {fields.get('reason') or ''}[/yellow]"
        )
    return fields


def _require_graph(root: Path, *, warn_stale: bool = True):
    from devcouncil.indexing.graph.build import load_code_graph

    graph = load_code_graph(root)
    if graph is None:
        status.print("[red]No code graph; run `dev map` first.[/red]")
        raise typer.Exit(code=1)
    if warn_stale:
        _warn_if_stale(root)
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
        from devcouncil.codeintel.build_control import writer_busy_details

        payload = {
            "ok": False,
            "code": "graph_writer_busy",
            "error": str(exc),
            **writer_busy_details(root),
        }
        if json_output:
            typer.echo(json.dumps(payload, indent=2))
        else:
            status.print(f"[red]{exc}[/red]")
            status.print(f"[dim]hint: {payload.get('hint') or 'dev map unlock'}[/dim]")
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
    if refresh.build_incomplete:
        # SQLite is intact but older than HEAD — never report a green index.
        payload = {
            "ok": False,
            "build_incomplete": True,
            "reason": refresh.reason,
            "mode": refresh.mode,
        }
        if json_output:
            typer.echo(json.dumps(payload, indent=2))
        else:
            status.print(
                f"[yellow]Graph build did not finish; indexed state came from the "
                f"last committed generation ({refresh.reason})[/yellow]"
            )
        raise typer.Exit(code=1)
    result = get_codeintel_service(root).status()
    if refresh.compatibility_export_degraded:
        from devcouncil.indexing.graph.communities import compatibility_export_limit

        result["limit"] = compatibility_export_limit(
            canonical_store_health=_canonical_store_health(root),
            reason=refresh.reason or "compatibility export degraded",
        ).as_dict()
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
        if refresh.compatibility_export_degraded and result.get("limit"):
            _emit_limit(status, result["limit"])


@app.command("status")
def graph_status(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Show canonical generation, watcher health, and pending files."""
    from devcouncil.codeintel import get_codeintel_service
    from devcouncil.codeintel.build_control import read_build_status, writer_busy_details
    from devcouncil.codeintel.sync import get_sync_coordinator
    from devcouncil.codeintel.sync.lease import read_holder

    root = _root(project_root)
    rust_status = _devmap_query_payload(root, "status")
    result = get_codeintel_service(root).status()
    if rust_status is not None:
        # Prefer Rust generation/freshness when the binary+DB are available.
        result = {**result, **{k: v for k, v in rust_status.items() if k != "sync"}}
        sync = result.get("sync") if isinstance(result.get("sync"), dict) else {}
        result["sync"] = {**sync, **(rust_status.get("sync") or {})}
        result["source"] = "devmap+python"
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
    holder = read_holder(root / ".devcouncil" / "codeintel" / "writer.lock")
    build = read_build_status(root)
    result["writer_holder"] = {
        "pid": holder.pid,
        "started_at": holder.started_at,
    }
    busy_like = build.state in {"building", "stalled", "timed_out", "stale"} or holder.pid is not None
    if busy_like:
        details = writer_busy_details(root)
        result["hint"] = details["hint"]
        result["build_pid"] = details["build_pid"]
        result["build_state"] = details["build_state"]
    if result["sync"].get("compatibility_export") == "degraded":
        from devcouncil.indexing.graph.communities import (
            collect_limit_reports,
            compatibility_export_limit,
        )

        result["limit"] = compatibility_export_limit(
            canonical_store_health=_canonical_store_health(root),
            reason=str(result["sync"].get("degraded_reason") or "compatibility export degraded"),
        ).as_dict()
        result["limits"] = collect_limit_reports(result.get("limit"))
    result["index_freshness"] = _index_freshness_fields(root)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    console.print(f"state: {result['state']}")
    console.print(f"generation: {result.get('generation') or '(none)'}")
    freshness = result["index_freshness"]
    if freshness.get("fresh") is False:
        console.print(f"[red]index: STALE — {freshness.get('reason')}[/red]")
    elif freshness.get("fresh") is True:
        head = str(freshness.get("index_head") or "")
        console.print(f"index: fresh (HEAD {head[:12]})")
    console.print(f"nodes/edges: {result.get('node_count', 0)}/{result.get('edge_count', 0)}")
    sync = result["sync"]
    console.print(f"watcher: {sync['state']} ({sync.get('backend') or 'not started'})")
    if sync.get("state") in {"disabled", "stopped", ""} or not sync.get("backend"):
        console.print(
            "[dim]hint: run `dev map watch` or `dev map --watch` to enable auto-refresh[/dim]"
        )
    if sync.get("build_id") or holder.pid is not None:
        progress = f"{sync.get('build_completed', 0)}/{sync.get('build_total', 0)}"
        pid = sync.get("build_pid") or holder.pid or "n/a"
        console.print(
            f"build: {sync.get('build_state') or 'unknown'} / "
            f"{sync.get('build_phase') or 'unknown'} ({progress}, "
            f"pid={pid})"
        )
    if holder.pid is not None:
        console.print(f"writer holder: pid={holder.pid}")
    if result.get("hint"):
        console.print(f"[dim]hint: if stuck, run `{result['hint']}`[/dim]")
    if sync.get("compatibility_export") == "degraded":
        console.print("compatibility export: degraded")
        if result.get("limit"):
            _emit_limit(console, result["limit"])
    if sync.get("pending"):
        console.print("pending: " + ", ".join(sync["pending"]))
    if sync.get("degraded_reason"):
        console.print(f"degraded: {sync['degraded_reason']}")


@app.command("unlock")
def graph_unlock(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    force: bool = typer.Option(
        False,
        "--force",
        help="Kill the holder even if build_status still looks like progress.",
    ),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Free a stuck code-intelligence writer lease (prefer over raw kill).

    Default recovery: free when the recorded holder is dead; if status is
    stalled/timed_out/stale or the holder is older than the stall timeout,
    SIGTERM then SIGKILL. Use ``--force`` to kill a still-progressing holder.
    """
    from devcouncil.codeintel.build_control import unlock_writer_lease

    root = _root(project_root)
    result = unlock_writer_lease(root, force=force)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
    else:
        action = str(result.get("action") or "unknown")
        reason = str(result.get("reason") or "")
        color = "green" if result.get("ok") else "yellow"
        status.print(f"[{color}]unlock {action}: {reason}[/{color}]")
        if result.get("target_pid") is not None:
            status.print(f"target pid: {result['target_pid']}")
        if result.get("build_state"):
            status.print(
                f"build: {result.get('build_state')} "
                f"(pid={result.get('build_pid') or result.get('holder_pid') or 'n/a'})"
            )
        if result.get("hint") and not result.get("ok"):
            status.print(f"[dim]hint: {result['hint']}[/dim]")
    if not result.get("ok"):
        raise typer.Exit(code=1)


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
    run_foreground_watch(_root(project_root), liveness=True, out=status)


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
    # The canonical graph and its JSON export are separate axes. A size-capped
    # export is an export-tier limit, not a broken graph: SQLite is canonical and
    # fully queryable, so it must not stamp the whole map as failed. Real
    # inconsistencies (drift / corrupt / missing-while-committed) still fail.
    graph_ok = store["state"] == "committed"
    json_export_ok = export_health == "healthy"
    export_only_size_capped = export_health == "degraded"
    result = {
        "ok": graph_ok
        and grammars["ok"]
        and (json_export_ok or export_only_size_capped),
        "graph_ok": graph_ok,
        "json_export_ok": json_export_ok,
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
    from devcouncil.indexing.graph.build import load_code_graph
    from devcouncil.indexing.graph.communities import (
        collect_limit_reports,
        compatibility_export_limit,
        community_detection_limit,
    )

    _doctor_limits: list[dict] = []
    _doctor_health = _canonical_store_health(root)
    if export_health != "healthy":
        _export_limit = compatibility_export_limit(
            canonical_store_health=_doctor_health,
            reason=export_detail or export_health,
        ).as_dict()
        result["compatibility_export"]["limit"] = _export_limit
        _doctor_limits.append(_export_limit)
    try:
        _graph = load_code_graph(root)
        _communities = ((_graph.meta or {}) if _graph is not None else {}).get("communities") or {}
        if isinstance(_communities, dict) and _communities.get("skipped"):
            raw_limit = _communities.get("limit")
            if isinstance(raw_limit, dict) and raw_limit.get("degraded"):
                _comm_limit = raw_limit
            else:
                _comm_limit = community_detection_limit(
                    canonical_store_health=_doctor_health,
                    reason=str(_communities.get("reason") or "community_detection_skipped"),
                ).as_dict()
            _doctor_limits.append(_comm_limit)
    except Exception:
        logger.debug("graph doctor community limit probe failed", exc_info=True)
    if _doctor_limits:
        result["limits"] = collect_limit_reports(*_doctor_limits)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        if not result["ok"]:
            raise typer.Exit(code=1)
        return
    console.print(f"store: {store['state']} (schema {store['schema_version']})")
    if result.get("store_action"):
        console.print(f"store action: {result['store_action']}")
    console.print(f"watcher backend: {watcher_backend}")
    console.print(f"graph (canonical SQLite): {'ok' if graph_ok else 'not ok'}")
    console.print(
        f"compatibility export: {export_health}"
        + (f" — {export_detail}" if export_detail else "")
        + (" (JSON export only; the graph itself is fine)" if export_only_size_capped else "")
    )
    for _limit in result.get("limits") or []:
        _emit_limit(console, _limit)
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
    semantic: bool = typer.Option(
        False,
        "--semantic",
        help="Rank by name similarity in the kernel instead of prefix matching.",
    ),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Full-text (or semantic) symbol and path search over the committed generation.

    ``--semantic`` no longer depends on a separately built embedding index. The
    kernel derives the ranking from the symbol names in the generation it
    already holds, so the flag either works or reports that no map exists —
    it can no longer quietly fall back to prefix matching while claiming to be
    semantic.
    """
    root = _root(project_root)
    result = _devmap_query_payload(root, "search", query=query, limit=limit, semantic=semantic)
    if result is None:
        if semantic:
            # No silent downgrade. Prefix matching is a different answer, and
            # returning it under a `--semantic` flag is the failure this used
            # to have: the old path fell through to keyword search whenever the
            # embedding index was missing, which it was by default.
            typer.secho(
                "semantic search is unavailable: it needs the devmap index "
                "(run `dev map` to build one)",
                fg=typer.colors.YELLOW,
                err=True,
            )
            raise typer.Exit(3)
        from devcouncil.codeintel.query import CodeIntelQueryEngine

        result = CodeIntelQueryEngine(root).search(query, limit=limit)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if result.get("limit"):
        _emit_limit(status, result["limit"])
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
            from devcouncil.codeintel.build_control import writer_busy_details

            payload = {
                "ok": False,
                "code": "graph_writer_busy",
                "error": str(exc),
                "paths": changed,
                **writer_busy_details(root),
            }
            if json_output:
                typer.echo(json.dumps(payload, indent=2))
            else:
                status.print(f"[red]{exc}[/red]")
                status.print(f"[dim]hint: {payload.get('hint') or 'dev map unlock'}[/dim]")
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
    payload = {
        # A map rebuilt from a prior generation after a build timeout is not a
        # successful ingest: the graph is intact but older than HEAD. Reporting
        # ok/green here is exactly the confusion the recovery path exists to
        # avoid, so it fails alongside `degraded`.
        "ok": not (refresh.degraded or refresh.build_incomplete),
        "paths": changed,
        "map": str(map_path.relative_to(root)),
        "generation": refresh.generation,
        "mode": refresh.mode,
        "degraded": refresh.degraded,
        "build_incomplete": refresh.build_incomplete,
        "reason": refresh.reason,
    }
    if refresh.compatibility_export_degraded:
        from devcouncil.indexing.graph.communities import compatibility_export_limit

        payload["compatibility_export"] = "degraded"
        payload["compatibility_export_reason"] = refresh.reason
        payload["limit"] = compatibility_export_limit(
            canonical_store_health=_canonical_store_health(root),
            reason=refresh.reason or "compatibility export degraded",
        ).as_dict()
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
    else:
        color = "yellow" if (refresh.degraded or refresh.build_incomplete) else "green"
        status.print(
            f"[{color}]Ingested {len(changed)} path(s); map at {payload['map']}"
            f"{f'; degraded: {refresh.reason}' if refresh.degraded else ''}"
            f"{f'; graph build did not finish — map came from the last committed '
               f'generation ({refresh.reason})' if refresh.build_incomplete else ''}[/{color}]"
        )
        if refresh.compatibility_export_degraded and payload.get("limit"):
            _emit_limit(status, payload["limit"])
    if refresh.degraded or refresh.build_incomplete:
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
    """Install an opt-in post-checkout/post-merge reconciliation hook.

    New installs prefer ``dev map sync``; existing ``dev graph sync`` hooks still
    work via the ``graph`` alias.
    """
    root = _root(project_root)
    git_dir = root / ".git"
    if not git_dir.is_dir():
        status.print("[red]Git hook installation requires a normal .git directory.[/red]")
        raise typer.Exit(code=1)
    hook_body = (
        "#!/bin/sh\n"
        'exec dev map sync --project-root "$(git rev-parse --show-toplevel)" >/dev/null 2>&1\n'
    )
    for name in ("post-checkout", "post-merge"):
        path = git_dir / "hooks" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists():
            existing = path.read_text(encoding="utf-8", errors="replace")
            if "dev map sync" not in existing and "dev graph sync" not in existing:
                status.print(f"[red]Refusing to overwrite existing hook: {path}[/red]")
                raise typer.Exit(code=1)
        path.write_text(hook_body, encoding="utf-8")
        path.chmod(0o755)
    status.print(
        "[green]Installed post-checkout and post-merge code-intelligence hooks "
        "(dev map sync).[/green]"
    )


@app.command("query")
def graph_query(
    name_or_path: str = typer.Argument(..., help="Symbol name or file path."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """360° view: definition, callers, callees, importers."""
    root = _root(project_root)
    result = _devmap_query_payload(root, "query", name_or_path=name_or_path)
    if result is None:
        from devcouncil.indexing.graph import query_symbol

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
        for field in ("callers", "callees", "importers"):
            console.print(f"  {field}: {_render_edge_field(d, field)}")


@app.command("trace")
def graph_trace(
    start: str = typer.Argument(..., help="Start node (name or path)."),
    end: str = typer.Argument(..., help="End node (name or path)."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Shortest path between two graph nodes."""
    root = _root(project_root)
    result = _devmap_query_payload(root, "trace", start=start, end=end)
    if result is None:
        from devcouncil.indexing.graph import trace_path

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
    allow_stale: bool = typer.Option(
        False,
        "--allow-stale",
        help="Report even when the index was built from a different commit "
        "than HEAD (default: exit 3 so stale results cannot pass as evidence).",
    ),
) -> None:
    """Full dead-code report with confidence tiers and reasons."""
    from collections import Counter

    from devcouncil.indexing.graph.liveness import confidence_at_least

    root = _root(project_root)
    freshness = _warn_if_stale(root, note_unknown=True, quiet=json_output)
    rust_dead = _devmap_query_payload(root, "dead")
    if rust_dead is not None:
        class _DeadEntry:
            def __init__(self, row: dict):
                self.id = row.get("id")
                self.path = row.get("path")
                self.line = row.get("line")
                self.kind = row.get("kind")
                self.reason = row.get("reason")
                self.confidence = row.get("confidence")

            def model_dump(self):
                return {
                    "id": self.id,
                    "path": self.path,
                    "line": self.line,
                    "kind": self.kind,
                    "reason": self.reason,
                    "confidence": self.confidence,
                }

        entries = [_DeadEntry(row) for row in rust_dead.get("dead_code") or []]
        # Skip Python graph load on successful Rust path.
        graph = None
    else:
        graph = _require_graph(root, warn_stale=False)
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
    stale = freshness.get("fresh") is False
    if json_output:
        typer.echo(
            json.dumps(
                {
                    "dead_code": [e.model_dump() for e in entries],
                    "dead_code_hidden": hidden,
                    "index_freshness": freshness,
                    **degraded,
                },
                indent=2,
            )
        )
        if stale and not allow_stale:
            raise typer.Exit(code=3)
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
        if stale and not allow_stale:
            status.print(
                "[red]refusing to treat a stale dead-code report as evidence "
                "(index built from a different commit than HEAD). Run `dev map` "
                "to refresh, or pass --allow-stale to accept.[/red]"
            )
            raise typer.Exit(code=3)
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
    if stale and not allow_stale:
        status.print(
            "[red]refusing to treat a stale dead-code report as evidence "
            "(index built from a different commit than HEAD). Run `dev map` "
            "to refresh, or pass --allow-stale to accept.[/red]"
        )
        raise typer.Exit(code=3)


@app.command("preview")
def graph_preview(
    file: str = typer.Argument(..., help="Repository-relative path the buffer would be written to."),
    content: Optional[Path] = typer.Option(
        None, "--content", help="File holding the candidate content; omit to read stdin."
    ),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    budget: int = typer.Option(2000, "--budget"),
    min_confidence: float = typer.Option(
        0.5,
        "--min-confidence",
        help="Confidence a call edge needs to be listed. The default excludes "
        "the resolver's name-only tier, which is counted separately.",
    ),
) -> None:
    """Ask what an unsaved edit would do to the graph, without writing it."""
    import sys

    from devcouncil.devmap_client import DevMapClientError, try_connect

    root = _root(project_root)
    _warn_if_stale(root, note_unknown=True, quiet=json_output)

    if content is not None:
        source = content.read_text(encoding="utf-8", errors="replace")
    else:
        source = sys.stdin.read()

    client = try_connect(root)
    if client is None:
        # No Python fallback: the delta comes from parsing the buffer with the
        # tree-sitter grammars the Rust kernel owns. Reporting nothing found
        # would read as "this edit changes nothing".
        message = (
            "preview is unavailable: it needs the devmap index "
            "(run `dev map` to build one)"
        )
        if json_output:
            typer.echo(json.dumps({"error": message}, indent=2))
        else:
            typer.secho(message, fg=typer.colors.YELLOW, err=True)
        raise typer.Exit(3)

    try:
        report = client.preview(
            file=file, content=source, budget=budget, min_confidence=min_confidence
        )
    except DevMapClientError as exc:
        typer.secho(f"preview failed: {exc}", fg=typer.colors.RED, err=True)
        raise typer.Exit(1) from exc

    if json_output:
        typer.echo(json.dumps(report, indent=2))
        return

    typer.echo(f"{report.get('file_path')}  parse={report.get('parse_status')}")
    if report.get("compared_against") == "nothing":
        typer.echo("note: no file at this path; every symbol reads as added")
    if not report.get("file_is_indexed"):
        typer.echo("note: this file is not in the index; no caller graph is available for it")
    if report.get("degraded_reason"):
        typer.echo(f"note: {report['degraded_reason']}")
    if not report.get("delta_available"):
        raise typer.Exit(3)

    labels = {
        "Added": "added",
        "Removed": "removed",
        "SignatureChanged": "signature",
        "BodyChanged": "body",
        "Changed": "changed",
    }
    for symbol in report.get("symbols") or []:
        change = labels.get(str(symbol.get("change")), str(symbol.get("change")))
        typer.echo(f"{change:<11} {symbol.get('qualified_name')} ({symbol.get('kind')})")
    if not (report.get("symbols") or []):
        typer.echo("no symbol-level change")
    if report.get("bodies_not_compared"):
        typer.echo(
            f"{report['bodies_not_compared']} symbol(s) declared identically but not "
            "body-compared (below the signature size floor, or no grammar)"
        )

    callers = (report.get("broken_callers") or {}).get("items") or []
    for caller in callers:
        typer.echo(
            f"  affects  {caller.get('caller_symbol')}  ->  "
            f"{caller.get('target_symbol')}  ({caller.get('confidence'):.2f})"
        )
    if not callers:
        typer.echo("no calls from other files are affected")
    if report.get("ambiguous_callers"):
        typer.echo(
            f"{report['ambiguous_callers']} further call edge(s) fell below the "
            "confidence floor and are not listed; pass --min-confidence 0 to see them"
        )


@app.command("savings")
def graph_savings(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    query: Optional[str] = typer.Option(None, "--query", help="Also account for one search."),
    budget: int = typer.Option(2000, "--budget"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Report what the map cost against what reading files would have.

    Every figure is an estimate of bytes / 4, and the comparison charges the
    alternative only for reading the files the map already named — so the
    reported saving is a floor, not a best case.
    """
    from devcouncil.devmap_client import DevMapClientError, try_connect

    root = _root(project_root)
    client = try_connect(root)
    if client is None:
        message = "savings is unavailable: it needs the devmap index (run `dev map` to build one)"
        if json_output:
            typer.echo(json.dumps({"error": message}, indent=2))
        else:
            typer.secho(message, fg=typer.colors.YELLOW, err=True)
        raise typer.Exit(3)
    try:
        report = client.savings(query=query, budget=budget)
    except DevMapClientError as exc:
        typer.secho(f"savings failed: {exc}", fg=typer.colors.RED, err=True)
        raise typer.Exit(1) from exc
    typer.echo(json.dumps(report, indent=2))


@app.command(
    "workspace",
    context_settings={"allow_extra_args": True, "ignore_unknown_options": True},
)
def graph_workspace(
    ctx: typer.Context,
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Manage the multi-repo registry: add, remove, list, search, links.

    Arguments are passed to `devmap workspace` unchanged, so this stays one
    command rather than a second copy of its subcommand tree that can drift.
    """
    from devcouncil.devmap_client import DevMapClientError, try_connect

    root = _root(project_root)
    client = try_connect(root)
    if client is None:
        message = (
            "workspace is unavailable: it needs the devmap binary "
            "(run `dev map` to build an index first)"
        )
        typer.secho(message, fg=typer.colors.YELLOW, err=True)
        raise typer.Exit(3)
    if not ctx.args:
        typer.secho("usage: dev map workspace <add|remove|list|search|links> ...", err=True)
        raise typer.Exit(2)
    try:
        result = client.workspace(list(ctx.args))
    except DevMapClientError as exc:
        typer.secho(f"workspace failed: {exc}", fg=typer.colors.RED, err=True)
        raise typer.Exit(1) from exc
    typer.echo(json.dumps(result, indent=2))


@app.command("clones")
def graph_clones(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    kind: Optional[str] = typer.Option(
        None, "--kind", help="Report only 'exact' or only 'structural' groups."
    ),
    min_nodes: int = typer.Option(
        0, "--min-nodes", help="Drop groups whose smallest body is under this many parse nodes."
    ),
    budget: int = typer.Option(2000, "--budget"),
) -> None:
    """Report duplicated symbol bodies.

    ``exact`` groups are the same code modulo formatting and comments;
    ``structural`` groups are the same shape under renaming, and cover callables
    only.
    """
    from devcouncil.devmap_client import DevMapClientError, try_connect

    root = _root(project_root)
    _warn_if_stale(root, note_unknown=True, quiet=json_output)

    client = try_connect(root)
    if client is None:
        # No Python fallback exists: body signatures are produced by the
        # tree-sitter extraction the Rust kernel owns, and there is nothing to
        # group without them. Saying so beats printing an empty report that
        # reads as "no duplicates found".
        message = (
            "clone detection is unavailable: it needs the devmap index "
            "(run `dev map` to build one)"
        )
        if json_output:
            typer.echo(json.dumps({"error": message, "groups": None}, indent=2))
        else:
            typer.secho(message, fg=typer.colors.YELLOW, err=True)
        raise typer.Exit(3)

    try:
        report = client.clones(budget=budget, kind=kind, min_nodes=min_nodes)
    except DevMapClientError as exc:
        typer.secho(f"clone query failed: {exc}", fg=typer.colors.RED, err=True)
        raise typer.Exit(1) from exc

    if json_output:
        typer.echo(
            json.dumps(
                {
                    "groups": report.groups.items,
                    "shown": report.groups.shown,
                    "hidden": report.groups.hidden,
                    "total": report.groups.total,
                    "truncated": report.groups.truncated,
                    "signed_symbols": report.signed_symbols,
                    "unsigned_symbols": report.unsigned_symbols,
                },
                indent=2,
            )
        )
        return

    for group in report.groups.items:
        members = group.get("members") or []
        typer.echo(
            f"{str(group.get('kind', '?')).lower()}  {len(members)} members  "
            f"{group.get('min_nodes', 0)} nodes"
        )
        for member in members:
            typer.echo(
                f"    {member.get('file_path')}:{member.get('span_start')}  "
                f"{member.get('symbol_name')}"
            )
        omitted = group.get("members_omitted") or 0
        if omitted:
            typer.echo(f"    ... {omitted} more members not listed")
    # Printed even when nothing was found, so "no duplicates" cannot be confused
    # with "nothing was examined".
    typer.echo(
        f"coverage: {report.signed_symbols} symbols signed, "
        f"{report.unsigned_symbols} unsigned"
    )
    if report.groups.truncated:
        typer.echo(
            f"showing {report.groups.shown} of {report.groups.total} groups "
            f"({report.groups.hidden} withheld by the token budget)"
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
    root = _root(project_root)
    if not diff and not paths:
        status.print("[red]Provide paths or --diff.[/red]")
        raise typer.Exit(code=1)
    result = None
    if not diff and paths:
        result = _devmap_query_payload(
            root, "impact", paths=list(paths), max_depth=max_depth
        )
    if result is None:
        from devcouncil.indexing.graph.intel import diff_impact

        graph = _require_graph(root)
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
        CompatibilityGraphTooLarge,
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
    export_warning = ""
    try:
        write_code_graph(root, graph, analysis_shards=merged)
    except CompatibilityGraphTooLarge as exc:
        # SQLite committed the PDG shards and a stub/pointer JSON is on disk;
        # only the compatibility export is degraded — not the PDG build.
        export_warning = str(exc)
    stats = (graph.meta.get("pdg") or {}).get("stats") or {}
    payload = {"ok": True, "stats": stats, "files": sorted(layer.files.keys())}
    if export_warning:
        payload["compatibility_export"] = "degraded"
        payload["compatibility_export_reason"] = export_warning
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
        return
    console.print(
        f"PDG: {stats.get('function_count', 0)} functions, "
        f"{stats.get('taint_count', 0)} taint findings across {stats.get('file_count', 0)} files"
    )
    if export_warning:
        console.print(f"[yellow]compatibility export degraded: {export_warning}[/yellow]")


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
