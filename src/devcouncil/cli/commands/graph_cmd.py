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
    """Watch the tree and rebuild through the Rust kernel until interrupted.

    `dev map watch` and `dev map --watch` used to be two watchers on two
    engines: this one drove the Python coordinator into `index.sqlite`, the
    other rebuilt the kernel store. Same word, opposite store. One seam now.
    """
    del out  # the map watcher reports on its own status console
    from devcouncil.cli.commands.map import _watch_map

    _watch_map(root, liveness=liveness)


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


def _emit_limit(out: Console, limit_dict: dict) -> None:
    if not limit_dict.get("degraded"):
        return
    out.print(f"[yellow]limit ({limit_dict.get('kind')}): {limit_dict.get('reason')}[/yellow]")
    if limit_dict.get("recovery_command"):
        out.print(f"[dim]recovery: {limit_dict['recovery_command']}[/dim]")
    if limit_dict.get("detail"):
        out.print(f"[dim]{limit_dict['detail']}[/dim]")


def _index_freshness_fields(root: Path) -> dict[str, object]:
    """Freshness probe for read commands; never raises.

    This asked ``codeintel.service.index_freshness``, which compared the Python
    store's committed generation against git HEAD. Nothing had committed a
    generation since the kernel became the only writer, so it returned
    ``{"fresh": None, "reason": "no committed index generation"}`` on every
    repository — and :func:`_warn_if_stale`'s "index STALE" banner, added after a
    frozen index misled a deletion decision, could not fire at all.

    ``devmap_health.map_freshness`` is the surviving owner of the same question:
    it asks whether ``repo_map.json`` is the map of the tree as it stands, by the
    same rule ``dev map --if-stale`` uses. ``age_seconds`` comes from the
    artifact this verdict is about.
    """
    try:
        from devcouncil.devmap_health import map_freshness

        fields: dict[str, object] = dict(map_freshness(root))
        try:
            from devcouncil.devmap_engine import DEFAULT_MAP_RELPATH

            fields["age_seconds"] = max(
                0.0, time.time() - (root / DEFAULT_MAP_RELPATH).stat().st_mtime
            )
        except OSError:
            fields["age_seconds"] = None
        return fields
    except Exception as exc:  # noqa: BLE001 - probe must not break reads
        return {"fresh": None, "reason": f"freshness probe failed: {exc}"}


#: Which edge kinds count as calls, and how to read one direction's edge list,
#: moved to :mod:`devcouncil.devmap_client` — the one seam every kernel consumer
#: already goes through — when the task prompt's impact block became a second
#: reader of the same raw edges. Re-bound under the private name this module's
#: own call sites already use. `CALL_EDGE_KINDS` itself had no reader outside
#: `edge_nodes` (`rg -uu` over `src/` and `tests/`), so it did not come along.
from devcouncil.devmap_client import edge_nodes as _edge_nodes  # noqa: E402


def _call_edges(
    client,
    method: str,
    target: str,
    symbol_key: str,
    file_key: str,
    min_rung: Optional[str] = None,
):
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
        # The floor is passed, not defaulted away: `deps`, `impact` and `trace`
        # all take one, and this dispatcher is the only thing between them and
        # the caller. A client that cannot apply it is refused rather than
        # asked without it — the refusal arrives below as `(None, reason)`,
        # which is this function's whole contract: not measured, and why.
        call = getattr(client, method)
        kwargs: dict[str, object] = {"depth": 1}
        if _sends_min_rung(call, method, min_rung):
            kwargs["min_rung"] = min_rung
        resp = call(target, **kwargs)
    except DevMapClientError as exc:
        return None, f"{method} failed: {exc}"

    reason = resolution_unavailable_reason(resp.resolution)
    if reason:
        return None, f"{method} resolution unavailable: {reason}"

    return _edge_nodes(resp.items, symbol_key, file_key), None


def _sends_min_rung(call, what: str, min_rung: Optional[str]) -> bool:
    """Whether to pass the floor to ``call`` — refusing if it cannot take one.

    Three-way on purpose. No floor asked for: send nothing, and the call is
    byte-identical to what every existing caller already makes. A floor the
    method accepts: send it. A floor it cannot accept: refuse, because the one
    outcome that must never happen is the request going out *without* the floor
    — that answers the broader question and hands back a wide list the caller
    reads as the narrow one they asked for.

    Probed rather than caught. An `except TypeError` around the call would also
    swallow one raised inside the response handling, which is a genuine shape
    error; that is the rule :func:`_neighbor_edges` already states for
    `AttributeError`, for the same reason.

    Older client shapes are a supported case here, not a hypothetical:
    `test_neighbors_batching.py` models a partially upgraded install, and the
    per-target path this routes back to has taken a floor since it landed.
    """
    if min_rung is None:
        return False
    import inspect

    from devcouncil.devmap_client import DevMapClientError

    if "min_rung" not in inspect.signature(call).parameters:
        raise DevMapClientError(
            f"this devmap client cannot apply a rung floor to {what}"
        )
    return True


def _batched_neighbors(batched, targets: list, min_rung: Optional[str]):
    """Call the batched command, passing the floor only when one was asked for."""
    if _sends_min_rung(batched, "the batched neighbors query", min_rung):
        return batched(targets, min_rung=min_rung)
    return batched(targets)


def _neighbor_edges(client, targets: list, min_rung: Optional[str] = None) -> dict:
    """Both directions for every target, in one kernel exchange where possible.

    Returns ``{target: (callers, callers_unavailable, callees,
    callees_unavailable)}`` with exactly the fail-closed contract
    :func:`_call_edges` documents: ``None`` for a direction that could not be
    measured, never ``[]``.

    The batched kernel command is an optimisation, not a requirement. A
    ``devmap`` binary predating it — an older release on ``PATH``, a partial
    upgrade — makes the request fail, and the per-target path answers instead.
    Slower, identical answers; the alternative is a hard error on a stale
    binary.
    """
    from devcouncil.devmap_client import (
        DevMapClientError,
        resolution_unavailable_reason,
        walk_incomplete_reason,
    )

    answers: dict = {}
    # Probed, not caught. An `except AttributeError` around the call would also
    # swallow one raised *inside* the response handling below — a genuine shape
    # error — and quietly answer from the slow path instead of failing. This
    # asks only the question that matters: does this client have the command?
    batched = getattr(client, "neighbors", None)
    if targets and callable(batched):
        try:
            # A floor honoured on one transport and dropped on the other is an
            # answer whose breadth depends on which code path happened to run —
            # so the batched call takes it too, and a client that cannot apply
            # it falls through to the per-target path below rather than
            # answering at full breadth.
            for entry in _batched_neighbors(batched, targets, min_rung):
                target = entry["target"]
                # Four slots, alternating: the edges for a direction (or None
                # when it could not be measured) then the reason (or None).
                sides: list[list[str] | str | None] = []
                # `method` is the kernel command each direction stands for,
                # and it is what the unavailable reason names — the batched and
                # per-target paths must be indistinguishable to a reader, down
                # to the wording, or "which code path answered" leaks into a
                # message that is supposed to describe the store.
                for side, method, symbol_key, file_key in (
                    ("callers", "impact", "source_symbol", "source_file"),
                    ("callees", "deps", "target_symbol", "target_file"),
                ):
                    resp = entry[side]
                    reason = resolution_unavailable_reason(resp.resolution)
                    if reason:
                        sides += [None, f"{method} resolution unavailable: {reason}"]
                        continue
                    nodes = _edge_nodes(resp.items, symbol_key, file_key)
                    # An empty list from a walk that stopped early is the case
                    # that misleads: it reads as "nothing calls this" when the
                    # honest answer is "I stopped looking". The kernel reports
                    # that in `walk_incomplete`, which the counters cannot carry
                    # (a walk withholding an unknown quantity would break
                    # `shown + hidden == total`). A non-empty list is left
                    # alone — the caller has real edges, and the signal is on
                    # the response for anyone who wants it.
                    #
                    # Read through the shared accessor rather than off the
                    # attribute: the MCP handlers apply the same rule, and two
                    # readings of one signal is how they start to disagree.
                    incomplete = walk_incomplete_reason(resp)
                    if not nodes and incomplete:
                        sides += [None, f"{method} walk incomplete: {incomplete}"]
                    else:
                        sides += [nodes, None]
                answers[target] = tuple(sides)
            return answers
        except DevMapClientError:
            answers.clear()

    for target in targets:
        callers, callers_unavailable = _call_edges(
            client, "impact", target, "source_symbol", "source_file", min_rung
        )
        callees, callees_unavailable = _call_edges(
            client, "deps", target, "target_symbol", "target_file", min_rung
        )
        # `trace` is likewise probed rather than assumed: a client old enough
        # to lack `neighbors` may lack this too, and a missing method must
        # leave the reason `deps` gave, not raise.
        if callees is None and callable(getattr(client, "trace", None)):
            # `deps` resolves a *file* path, so for a symbol id it always
            # answers "not indexed". The kernel picks between `deps` and the
            # symbol-scoped forward traversal by asking the store what the
            # target is; here that is not observable, so the outcome stands in
            # for it. Without this, a stale binary would report every symbol's
            # callees as unknown while the batched path reported them — two
            # paths that silently answer differently is worse than one that is
            # merely slower.
            traced, _traced_unavailable = _call_edges(
                client, "trace", target, "target_symbol", "target_file", min_rung
            )
            if traced is not None:
                callees, callees_unavailable = traced, None
        answers[target] = (
            callers,
            callers_unavailable,
            callees,
            callees_unavailable,
        )
    return answers


#: Definitions per `dev map query` whose caller/callee edges are measured.
#: Each costs two kernel round-trips; beyond this the lists are reported as
#: unmeasured instead of spending 40 calls on partial matches.
_QUERY_EDGE_DEFINITION_CAP = 5


def _is_exact_query_match(item: dict, query: str) -> bool:
    """Whether a search hit *is* the queried symbol rather than a partial match."""
    name = str(item.get("symbol_name") or "")
    path_s = str(item.get("file_path") or "")
    node_id = f"{path_s}::{name}" if path_s and name else name or path_s
    wanted = query.strip()
    return bool(wanted) and (
        name == wanted
        or node_id == wanted
        or node_id.endswith(f"::{wanted}")
        or (not name and path_s == wanted)
    )


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


#: `--min-rung`, worded once for every command that offers it.
#:
#: A name rather than a `--min-confidence` float because the ladder has named
#: rungs and a caller wanting deterministic-only edges should not have to know
#: that means 1.0. Validated by `DevMapClient`, which refuses an unknown name
#: rather than sending the request without it — a dropped floor produces a
#: *broad* answer the caller reads as narrow.
_MIN_RUNG_HELP = (
    "Keep only edges at this resolution rung or stronger: deterministic, high, "
    "or speculative. Omitted filters nothing."
)


def _checked_min_rung(min_rung: Optional[str]) -> Optional[str]:
    """Refuse an unknown rung name here, before the request is attempted.

    `DevMapClient` validates too, but it raises `DevMapClientError` — and
    `_devmap_query_payload` catches that and returns `None`, which every caller
    reads as "no kernel available". A typo would therefore be reported as a
    missing index and the caller would go build one, twice, and still not get
    their filter. The names are read from the client so this cannot drift from
    what the kernel accepts.
    """
    from devcouncil.devmap_client import MIN_RUNG_NAMES

    if min_rung is None or min_rung in MIN_RUNG_NAMES:
        return min_rung
    status.print(
        f"[red]--min-rung must be one of {', '.join(MIN_RUNG_NAMES)}; "
        f"got {min_rung!r}[/red]"
    )
    raise typer.Exit(code=2)


def _devmap_query_payload(root: Path, kind: str, **kwargs):
    """The kernel's answer for a query surface, or ``None`` when it cannot answer.

    ``None`` used to mean "fall back to the Python engine". There is no Python
    engine for `query` or `trace` any more; ``None`` now means the caller
    refuses, naming the kernel. The signal is unchanged so the surfaces that
    still branch on it (`status`, and the MCP siblings) read it the same way.
    """
    from devcouncil.devmap_client import (
        DevMapClientError,
        DevMapRequestRefused,
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
            resp = client.trace(
                start, depth=3, to_symbol=end, min_rung=kwargs.get("min_rung")
            )
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
            # Edge lists cost two kernel round-trips per definition (`impact`
            # and `deps`), and a bare name matches partially all over a large
            # graph: one `dev map query` issued 41 calls, most of them for
            # symbols the caller never asked about. Exact matches come first
            # and only the first few definitions get their edges measured;
            # the rest say so, rather than pretending to have been measured.
            ranked = sorted(
                resp.items[:20],
                key=lambda item: 0 if _is_exact_query_match(item, name_or_path) else 1,
            )
            # One exchange for every measured definition, not two per
            # definition. `graph_query` was the last Dev Map read path that did
            # not get faster when the store did, and this fan-out was why: the
            # client falls back to a `devmap` subprocess whenever no daemon
            # socket is live, so five definitions meant eleven process spawns.
            #
            # Symbol-scoped on both sides. The outbound side used to be asked
            # about `path_s` — the FILE — so a function's "callees" were the
            # whole file's outbound edges: `IsRestatement` reported 35 callees
            # where the symbol-scoped answer is 0, and the list included
            # `Contains`/`MemberOf` structural edges, which is why symbols
            # appeared to call themselves.
            def _target_of(item: dict) -> str:
                path_s = str(item.get("file_path") or "")
                name = str(item.get("symbol_name") or "")
                node_id = f"{path_s}::{name}" if path_s and name else name or path_s
                return node_id if name else path_s or name_or_path

            measured = ranked[:_QUERY_EDGE_DEFINITION_CAP]
            # De-duplicated: two hits can share a target, and asking twice
            # spends a slot in a bounded batch on an answer already held.
            batch: list = []
            for item in measured:
                target = _target_of(item)
                if target not in batch:
                    batch.append(target)
            edges_by_target = _neighbor_edges(client, batch, kwargs.get("min_rung"))

            for position, item in enumerate(ranked):
                path_s = str(item.get("file_path") or "")
                name = str(item.get("symbol_name") or "")
                span = item.get("span") or (0, 0)
                line = int(span[0]) if isinstance(span, (list, tuple)) and span else 0
                node_id = f"{path_s}::{name}" if path_s and name else name or path_s
                target = _target_of(item)
                if position < _QUERY_EDGE_DEFINITION_CAP:
                    (
                        callers,
                        callers_unavailable,
                        callees,
                        callees_unavailable,
                    ) = edges_by_target[target]
                else:
                    callers, callees = None, None
                    callers_unavailable = callees_unavailable = (
                        f"not measured: beyond the first {_QUERY_EDGE_DEFINITION_CAP} "
                        "definitions; query the symbol by its full id"
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
                # The other half of the answer. A one-hop inbound-edge join
                # cannot see a subsystem whose functions call each other, so
                # `dead_code` without this is not a shorter list — it is a list
                # missing an entire class of finding.
                "dead_clusters": resp.dead_clusters,
                "dead_clusters_truncated": resp.dead_clusters_truncated,
                # The third outcome. A refused scan sends no list, so without
                # this key the payload would say only "not recorded" and the
                # reader would be told to rebuild, which refuses again.
                "dead_clusters_incomplete": resp.dead_clusters_incomplete,
                "source": "devmap",
                "truncated": resp.truncated,
                "total": resp.total,
                **_graph_degraded_fields(root),
            }
        if kind == "impact":
            # One renderer for a banded blast radius, shared with
            # `devcouncil_code_explore` / `_affected`. The alternative is a
            # second place that decides what `confidence` on a band means, and
            # the last time there were two, this one meant "distance" and the
            # other meant "evidence".
            from devcouncil.integrations.mcp.handlers.codeintel import (
                _blast_radius_payload,
            )

            paths = [str(p).replace("\\", "/") for p in (kwargs.get("paths") or [])]
            depth = max(1, min(3, int(kwargs.get("max_depth", 3))))
            items = []
            for path_s in paths:
                # `layers=True`: the bands are the kernel's, computed over the
                # edges of the walk it just performed. What stood here derived
                # them from the returned edge list, which cannot carry distance,
                # and so published every symbol a depth-3 walk reached as
                # `depth: 1, confidence: "extracted"`.
                resp = client.impact(path_s, depth=depth, layers=True)
                blast = _blast_radius_payload(resp.blast_radius or {})
                # The seeds are what the walk started from: the symbols in this
                # file that the generation holds an inbound edge for, plus the
                # file node itself. Not "every symbol defined here" — the key
                # says which, because an empty list used to read as the latter.
                items.append({
                    "path": path_s,
                    "symbols": [
                        {
                            "id": seed,
                            "path": seed.split("::", 1)[0],
                            "name": seed.split("::", 1)[1] if "::" in seed else seed,
                        }
                        for seed in blast.get("seeds") or []
                    ],
                    "symbols_are_walk_seeds": True,
                    "blast": blast,
                    # A target with no indexed inbound edge is an *answer* —
                    # "nothing reaches this" — and it used to be raised as a
                    # client error, which sent the whole command to
                    # `load_code_graph`. A correct answer must not trigger a
                    # whole-graph read.
                    "unavailable": blast.get("unavailable")
                    or resolution_unavailable_reason(resp.resolution),
                    # The edge half's own counters, so "3 callers listed" can
                    # never be read as "3 callers exist".
                    "edges_shown": resp.shown,
                    "edges_total": resp.total,
                    "edges_truncated": resp.truncated,
                    "resolution": "devmap",
                })
            # No `aggregate`. The retired Python engine computed one blast over
            # every path's seeds at once, which is not the union of the per-path
            # radii — a node two hops from one path can be one hop from another
            # — and the kernel answers one target per call, so a union published
            # under that name would be a different number wearing it. Nothing
            # reads the key: `rg -uu aggregate` finds only the producer, and the
            # kernel branch that has served this command since the M2 lane never
            # emitted it.
            return {
                "ok": True,
                "paths": items,
                "path_count": len(items),
                "source": "devmap",
                **_graph_degraded_fields(root),
            }
    except DevMapRequestRefused as exc:
        # The request, not the kernel, was refused: over the byte cap, not
        # UTF-8, a depth out of range. No engine can serve it, so it is an
        # error to the caller — never `None`, which would re-run it on the
        # Python graph engine and answer from a whole-graph load.
        return {"ok": False, "error": str(exc), "source": "devmap"}
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
    elif note_unknown and fields.get("fresh") is None:
        # `generation is not None` used to gate this, meaning "we have an index
        # but cannot judge it". The generation came from the Python store and
        # was always None, so the branch never ran. The verdict now comes from
        # the map artifact, and `fresh is None` is exactly "could not judge".
        status.print(
            f"[yellow]index freshness unknown: {fields.get('reason') or ''}[/yellow]"
        )
    return fields


def _require_graph(root: Path, *, warn_stale: bool = True):
    """The kernel's whole graph, read from the artifact the kernel writes.

    This went through `load_code_graph` — the retired engine's read path, which
    imports `code_graph.json` into the Python `index.sqlite` cache on first read
    (a ~94 MB write, under a writer lease, from a command that only reads) and
    re-materialises every node and edge out of SQLite afterwards. The commands
    below want the whole graph and the whole graph is already on disk.
    """
    from devcouncil.indexing.graph.build import GRAPH_INCOMPLETE_META, read_code_graph

    graph = read_code_graph(root)
    if graph is None:
        status.print(
            "[red]No code graph at .devcouncil/graph/code_graph.json; "
            "run `dev map` first.[/red]"
        )
        raise typer.Exit(code=1)
    # Named, never silent. A capped export answers in the shape of a complete
    # one, and the store that used to hide the cap is no longer in the path.
    incomplete = (graph.meta or {}).get(GRAPH_INCOMPLETE_META)
    if incomplete:
        status.print(f"[yellow]{incomplete}[/yellow]")
    if warn_stale:
        _warn_if_stale(root)
    return graph


#: Printed when no kernel can answer. Named so `query` and `trace` cannot drift
#: into saying different things about the same condition.
_NO_KERNEL_MESSAGE = (
    "[red]No devmap store; run `dev map` first. "
    "The kernel is the only graph engine — there is no Python fallback.[/red]"
)


def _require_kernel(root: Path, *, warn_stale: bool = True):
    """A live client, or exit naming the kernel — the sibling of `_require_graph`.

    For the commands the kernel answers directly. It reads the store the kernel
    wrote, rather than materialising the whole graph out of the Python
    `index.sqlite` cache first.
    """
    from devcouncil.devmap_client import try_connect

    client = try_connect(root)
    if client is None:
        status.print("[red]No devmap store; run `dev map` first.[/red]")
        raise typer.Exit(code=1)
    if warn_stale:
        _warn_if_stale(root)
    return client


def _kernel_build_payload(refresh) -> dict:  # noqa: ANN001
    """The JSON a kernel-backed build command reports."""
    kernel = getattr(refresh, "kernel_status", None)
    payload: dict = {
        "ok": not getattr(refresh, "degraded", False),
        "generation": getattr(refresh, "generation", None),
        "mode": getattr(refresh, "mode", "devmap-rust"),
        "degraded": bool(getattr(refresh, "degraded", False)),
        "reason": getattr(refresh, "reason", "") or "",
    }
    if kernel is not None:
        payload["node_count"] = kernel.node_count
        payload["edge_count"] = kernel.edge_count
        payload["kernel"] = {
            "is_fresh": kernel.is_fresh,
            "pending_count": kernel.pending_count,
            "quarantined_count": kernel.quarantined_count,
            "degraded_reason": kernel.degraded_reason,
        }
    return payload


def _run_kernel_build(
    root: Path,
    *,
    json_output: bool,
    label: str,
    full: bool = False,
    paths: Optional[List[str]] = None,
) -> dict:
    """Shared body of `init` / `ingest` / `sync`: one writer, one report.

    A kernel that cannot build ends the command red with the engine's own
    explanation (which names the binary and the fix); there is no fallback.
    """
    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    map_path = root / ".devcouncil" / "repo_map.json"
    try:
        refresh = refresh_map_artifacts(root, map_path, quiet=True, full=full)
    except DevMapEngineError as exc:
        payload = {"ok": False, "code": "engine_unavailable", "error": str(exc)}
        if paths is not None:
            payload["paths"] = list(paths)
        if json_output:
            typer.echo(json.dumps(payload, indent=2))
        else:
            status.print(f"[red]{label} failed: {exc}[/red]")
        raise typer.Exit(code=1) from exc
    payload = _kernel_build_payload(refresh)
    payload["map"] = str(map_path.relative_to(root))
    if paths is not None:
        payload["paths"] = list(paths)
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
    else:
        color = "green" if payload["ok"] and not payload["reason"] else "yellow"
        status.print(
            f"[{color}]{label}: generation {payload.get('generation') or '(none)'} — "
            f"{payload.get('node_count', 0)} nodes, {payload.get('edge_count', 0)} edges"
            f"{f'; {payload['reason']}' if payload['reason'] else ''}[/{color}]"
        )
    if not payload["ok"]:
        raise typer.Exit(code=1)
    return payload


@app.command("init")
def graph_init(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    full: bool = typer.Option(False, "--full", help="Force a cold rebuild in the kernel."),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Build the kernel store and write repo_map.json + code_graph.json."""
    _run_kernel_build(_root(project_root), json_output=json_output, label="Indexed", full=full)


@app.command("status")
def graph_status(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Engine, store, kernel freshness, daemon and artifact state of the map.

    Reads only what the Rust kernel owns. This used to merge the Python
    engine's `index.sqlite` status (generation 76, "STALE", a dead writer pid)
    over the kernel's, and described an engine `dev map` no longer uses.
    """
    from devcouncil.devmap_health import collect_map_status, render_status

    root = _root(project_root)
    result = collect_map_status(root)
    if json_output:
        typer.echo(json.dumps(result, indent=2, default=str))
        return
    for line in render_status(result):
        console.print(line, markup=False, highlight=False)


@app.command("sync")
def graph_sync(
    paths: Optional[List[str]] = typer.Argument(None, help="Optional paths; otherwise reconcile the project."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Rebuild the map now through the kernel (incremental when the tree allows).

    ``paths`` are accepted for callers that pass them and reported back; the
    kernel computes the affected set from content hashes itself, so an explicit
    list cannot narrow a build below what correctness requires.
    """
    _run_kernel_build(
        _root(project_root), json_output=json_output, label="Synced", paths=list(paths or [])
    )


@app.command("watch")
def graph_watch(
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Watch the tree and rebuild through the kernel until interrupted."""
    run_foreground_watch(_root(project_root), liveness=True, out=status)


@app.command("doctor")
def graph_doctor(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    fix: bool = typer.Option(
        False,
        "--fix",
        help="Apply every fix the doctor can apply from inside the repository, then re-check.",
    ),
) -> None:
    """Check the kernel binary, its store, the artifacts and their freshness.

    Every check carries a stable `code`, a sentence for a person (`fix`) and
    the exact command for an agent (`fix_command`). Critical failures (no
    usable kernel, a store newer than the kernel, an artifact written by a
    foreign engine) exit 1; reclaim pressure, a large WAL, a stale map,
    quarantined paths, a stuck build and a failed last build warn.

    `--fix` clears a dead build's marker, quarantines an unreadable store,
    drops stuck queue rows and runs one build for everything a build resolves;
    it never touches a running build and never rebuilds the kernel binary.
    """
    from devcouncil.devmap_health import apply_fixes, render_doctor, render_fixes, run_doctor

    root = _root(project_root)
    if fix:
        result = apply_fixes(root)
        if json_output:
            typer.echo(json.dumps(result, indent=2, default=str))
        else:
            for line in render_fixes(result):
                console.print(line, markup=False, highlight=False)
        if not result["ok"]:
            raise typer.Exit(code=1)
        return
    result = run_doctor(root)
    if json_output:
        typer.echo(json.dumps(result, indent=2, default=str))
    else:
        for line in render_doctor(result):
            console.print(line, markup=False, highlight=False)
    if not result["ok"]:
        raise typer.Exit(code=1)


@app.command("runs")
def graph_runs(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    last: int = typer.Option(10, "--last", min=1, max=200, help="How many runs to show."),
    failed: bool = typer.Option(False, "--failed", help="Only runs that failed."),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Records of recent kernel runs: what ran, how long, how it ended, and why.

    Every `devmap build` / `manifest` / `repair` the seam launches is recorded
    in the project trace log (`.devcouncil/logs/traces.jsonl`, the same log
    `devcouncil_tail_trace` reads) with its argv, exit code, duration, the
    kernel's notes (discovery refusals, reclaim, progress) and, on failure,
    the diagnosis code. The run id printed by a failing `dev map` is the key.
    """
    from devcouncil.devmap_engine import read_runs
    from devcouncil.devmap_health import _iso

    root = _root(project_root)
    runs = read_runs(root, limit=last, failed_only=failed)
    if json_output:
        typer.echo(json.dumps({"ok": True, "runs": runs}, indent=2, default=str))
        return
    if not runs:
        console.print("no kernel runs recorded yet", markup=False)
        return
    for run in runs:
        when = _iso(float(run["started_at"])) if run.get("started_at") else run.get("timestamp")
        verdict = "ok" if run.get("ok") else f"FAIL {run.get('code')}"
        console.print(
            f"{when} {run.get('stage', ''):<9} {verdict:<28} {run.get('duration_s', 0):>7}s "
            f"exit {run.get('exit_code')} run {run.get('run_id')}",
            markup=False,
            highlight=False,
        )
        for note in (run.get("notes") or [])[-4:]:
            console.print(f"    {note.strip()}", markup=False, highlight=False)


@app.command("abort")
def graph_abort(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Stop the kernel build that is running for this repository.

    SIGTERM, then SIGKILL after five seconds. Safe: a generation is one
    transaction, so the store stays on the prior generation and the OS
    releases the writer lock. Refuses a pid that is not a devmap process.
    """
    from devcouncil.devmap_health import abort_build

    root = _root(project_root)
    result = abort_build(root)
    if json_output:
        typer.echo(json.dumps(result, indent=2, default=str))
    elif result.get("aborted"):
        console.print(
            f"aborted build pid {result['pid']} with {result['signal']} (run {result.get('run_id')})",
            markup=False,
        )
    else:
        console.print(f"{result.get('code')}: nothing aborted", markup=False)
    if not result.get("ok"):
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
        # No second engine, and no silent downgrade. The Python
        # `CodeIntelQueryEngine` that answered plain search here read
        # `.devcouncil/codeintel/index.sqlite`, which nothing has written since
        # the Python writer was retired — so it did not answer, it raised. An
        # unbuilt map is reported as one.
        #
        # The two messages stay distinct: `--semantic` and plain search fail for
        # the same reason but a user who asked for one must not read a refusal
        # of the other as evidence that the flag is what broke.
        label = "semantic search" if semantic else "search"
        typer.secho(
            f"{label} is unavailable: it needs the devmap index "
            "(run `dev map` to build one)",
            fg=typer.colors.YELLOW,
            err=True,
        )
        raise typer.Exit(3)
    if result.get("error"):
        # A refused request (over the byte cap, not UTF-8). Rendering it as an
        # empty match list would report "nothing found" for a search that
        # never ran.
        if json_output:
            typer.echo(json.dumps(result, indent=2))
        else:
            typer.secho(str(result["error"]), fg=typer.colors.RED, err=True)
        raise typer.Exit(1)
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
    """Rebuild the map through the kernel and report the generation it committed.

    ``paths`` are reported back for callers that pass them; the kernel derives
    the affected set from content hashes itself, so the build is incremental
    exactly when the tree allows and never narrower than correctness needs.
    ``--no-liveness`` is accepted for old callers and ignored: the kernel
    always computes liveness.
    """
    del no_liveness
    _run_kernel_build(
        _root(project_root), json_output=json_output, label="Ingested", paths=list(paths or [])
    )


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
    """Return source, related symbols, paths, and blast radius in one query.

    Answered by the kernel. The Python engine this used to call loaded the whole
    graph into process memory to walk it, and since the Python writer was
    retired it had no store to load at all.
    """
    from devcouncil.devmap_client import DevMapClientError, try_connect

    client = try_connect(_root(project_root))
    if client is None:
        typer.secho(
            "explore is unavailable: it needs the devmap index "
            "(run `dev map` to build one)",
            fg=typer.colors.YELLOW,
            err=True,
        )
        raise typer.Exit(3)
    try:
        result = client.explore(query, limit=limit)
    except DevMapClientError as exc:
        typer.secho(f"explore is unavailable: {exc}", fg=typer.colors.YELLOW, err=True)
        raise typer.Exit(3) from exc
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    definitions = result.get("definitions") or {}
    for definition in definitions.get("items") or []:
        span = definition.get("span") or (0, 0)
        console.print(
            f"[bold]{definition['id']}[/bold] {definition['file_path']}:{span[0]}"
        )
        # Class A: a file that could not be read is reported as unread, never
        # rendered as a symbol whose body happens to be empty.
        reason = definition.get("source_unavailable_reason")
        if reason:
            console.print(f"  [yellow]source unavailable:[/yellow] {reason}")
        elif definition.get("source"):
            console.print(definition["source"])
        callers = definition.get("callers") or {}
        callees = definition.get("callees") or {}
        console.print(
            f"  callers={callers.get('shown', 0)}/{callers.get('total', 0)}"
            f" callees={callees.get('shown', 0)}/{callees.get('total', 0)}"
        )
    console.print(
        f"shown {definitions.get('shown', 0)} of {definitions.get('total', 0)}"
    )


@app.command("affected")
def graph_affected(
    targets: List[str] = typer.Argument(..., help="Symbol or path targets."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Find tests reachable through the inbound blast radius.

    Kernel-answered, ranked nearest-first. A target that matched nothing is
    named rather than folded into "no affected tests" — which is what the
    Python engine's empty seed set used to produce for a typo.
    """
    from devcouncil.devmap_client import DevMapClientError, try_connect

    client = try_connect(_root(project_root))
    if client is None:
        typer.secho(
            "affected is unavailable: it needs the devmap index "
            "(run `dev map` to build one)",
            fg=typer.colors.YELLOW,
            err=True,
        )
        raise typer.Exit(3)
    try:
        result = client.affected_tests(list(targets))
    except DevMapClientError as exc:
        typer.secho(f"affected is unavailable: {exc}", fg=typer.colors.YELLOW, err=True)
        raise typer.Exit(3) from exc
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    unmatched = (result.get("blast_radius") or {}).get("unmatched_targets") or []
    if unmatched:
        console.print(
            f"[yellow]no indexed traversal start for:[/yellow] {', '.join(unmatched)}"
        )
    tests = result.get("tests") or {}
    rows = tests.get("items") or []
    if not rows:
        console.print("No affected tests found.")
    for row in rows:
        console.print(f"{row['path']}  depth {row['depth']}")
    # Raw on purpose: the kernel is the only producer (no client, no answer —
    # exit 3 above), and ``DevMapClient._budgeted`` has already refused any
    # ``shown``/``total`` that is not a non-negative ``int``. See the note
    # above ``_string_members``.
    console.print(f"shown {tests.get('shown', 0)} of {tests.get('total', 0)}")


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
    min_rung: Optional[str] = typer.Option(None, "--min-rung", help=_MIN_RUNG_HELP),
) -> None:
    """360° view: definition, callers, callees, importers.

    --min-rung narrows every edge list here to the resolution rungs it names.

    The kernel is the only engine. This used to fall back to
    `indexing.graph.query.query_symbol` over `load_code_graph` — the retired
    engine's whole-graph read, 855 ms and ~690 MB RSS on this repository — and
    that fallback carried no resolution ladder, so `--min-rung` had to be
    refused against it separately. Both refusals are now the same one.
    """
    root = _root(project_root)
    min_rung = _checked_min_rung(min_rung)
    result = _devmap_query_payload(
        root, "query", name_or_path=name_or_path, min_rung=min_rung
    )
    if result is None:
        status.print(_NO_KERNEL_MESSAGE)
        raise typer.Exit(code=3 if min_rung is not None else 1)
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
    min_rung: Optional[str] = typer.Option(None, "--min-rung", help=_MIN_RUNG_HELP),
) -> None:
    """Shortest path between two graph nodes.

    --min-rung restricts the walk to the named rungs, so a path can be asked
    for on evidence the resolver proved rather than on evidence it guessed.

    The kernel is the only engine, and here that is a correctness rule rather
    than a performance one: the Python `trace_path` this fell back to ran an
    *undirected* BFS over `imports`/`calls`/`contains`/`defines`/`inherits`
    while the kernel walks resolved edges directionally. On a real probe Python
    reported a two-hop path between two functions through a shared test module
    where the kernel correctly reported none. A fabricated path is worse than an
    absent answer, because a caller acts on it.
    """
    root = _root(project_root)
    min_rung = _checked_min_rung(min_rung)
    result = _devmap_query_payload(
        root, "trace", start=start, end=end, min_rung=min_rung
    )
    if result is None:
        status.print(_NO_KERNEL_MESSAGE)
        raise typer.Exit(code=3 if min_rung is not None else 1)
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
    # Raw on purpose: both producers build ``path`` as ``list[str]`` — the
    # kernel branch of ``_devmap_query_payload`` wraps each node in ``str()``,
    # and ``trace_path`` walks ids ``CodeGraph`` typed at load. See the note
    # above ``_string_members``.
    console.print(" → ".join(result.get("path") or []))


# Where the same raw pattern is left alone, and why.
#
# The two helpers below exist because ``dead_clusters`` is the one payload
# ``CodeGraph`` declares untyped (``List[Dict[str, Any]]``), so a hand-edited
# ``code_graph.json`` reaches ``_render_dead_clusters`` exactly as written.
# Five other renderers in this file join or format a bare ``.get()`` the same
# way — ``graph_affected``, ``graph_trace``, ``graph_check_cmd``,
# ``graph_process`` and ``graph_routes`` — and, driven with the same shapes,
# every one of them misbehaved as the cluster renderer did: an ``int`` or a
# ``[1]`` raised out of the whole render, and a ``str`` joined into members
# that do not exist. They are not converted, because those shapes cannot
# arrive. Every graph reaches them through ``load_code_graph``, which refuses
# the file with ``CodeGraph.model_validate`` before any renderer sees it (the
# store path validates per row), and what they render is derived from fields
# the model types — ``GraphNode.id``, ``GraphEdge.source``/``target``,
# ``entry_roots`` — never from a field it leaves as ``Any``. ``graph_affected``
# has no graph path at all; its counters pass ``DevMapClient._budgeted``
# first. Pinned in ``tests/unit/test_graph_cmd_command.py`` by
# ``test_a_malformed_graph_file_is_refused_at_load_not_rendered`` and the two
# tests beside it.
#
# The rule that keeps this true: a renderer that reads a field the model
# leaves untyped — ``extras``, ``meta``, ``dead_clusters`` — goes through the
# helpers; one that reads a typed field may stay raw.


def _string_members(value: object) -> list[str]:
    """Members as a list of strings, or none at all.

    ``members`` is a ``Vec<String>`` on the wire (`dead_clusters.rs:80`), but
    this renderer does not read the wire — it reads ``code_graph.json`` off
    disk, which a half-written build, a truncated copy or a hand edit can leave
    in any shape. Two shapes escaped: a non-iterable raised ``TypeError`` out of
    the whole render, taking the well-formed findings beside it, and a *string*
    did not raise at all — ``[str(m) for m in "abc"]`` is ``["a", "b", "c"]``, so
    the reader saw a three-symbol dead cluster that does not exist. Fabricating
    a finding is the worse of the two, and it is the one nothing would have
    reported.
    """
    if not isinstance(value, (list, tuple)):
        return []
    return [str(member) for member in value]


def _whole_number(value: object, fallback: int) -> int:
    """A count as declared, or the fallback — never an exception.

    ``int("many")`` raises ``ValueError`` and ``int([1])`` raises ``TypeError``;
    both used to escape ``dev map dead``. A ``bool`` is rejected because
    ``int(True)`` is ``1``, which would print a one-symbol cluster from a field
    that carried no count at all.

    An integral float is accepted. JSON has one number type, so a hand edit or a
    non-Rust writer can spell a count ``3.0``, and the fallback here is
    ``len(members)`` — the *sample* length, capped at 25 — so refusing it would
    under-report the size of a dead subsystem, which is the specific mistake the
    size field exists to prevent.
    """
    if isinstance(value, bool):
        return fallback
    if isinstance(value, int):
        return value
    if isinstance(value, float) and value.is_integer():
        return int(value)
    return fallback


def _render_dead_clusters(
    console_, clusters: Optional[list], truncated: int, incomplete: Optional[str] = None
) -> None:
    """One line per abandoned cycle, with a member sample and the real size.

    Reported beside the single-symbol list rather than inside it: a forty-symbol
    dead subsystem is *one* thing a reader acts on, and forty rows would push
    real single-symbol findings past the display. The size printed is the true
    membership; the names are a sample and say so.

    ``None`` and ``[]`` print differently on purpose. "No abandoned cycles" is a
    finding. "This generation predates the pass" is not, and a reader deciding
    whether to rebuild needs to know which one they are looking at.

    ``incomplete`` is the third case and is checked first: the kernel ran the
    pass and refused it, so the rebuild advice below is not merely unhelpful
    but wrong — the next build walks the same graph and refuses again.
    """
    sample_cap = 4
    console_.print("")
    if incomplete:
        console_.print(f"Abandoned cycles: not computed — {incomplete}")
        return
    if clusters is None:
        console_.print(
            "Abandoned cycles: not recorded for this generation "
            "(run `dev map` to compute them)."
        )
        return
    if not clusters:
        console_.print("Abandoned cycles: none.")
        return
    console_.print(
        f"Abandoned cycles: {len(clusters)} component(s) nothing outside reaches."
    )
    for cluster in clusters:
        if not isinstance(cluster, dict):
            continue
        members = _string_members(cluster.get("members"))
        size = _whole_number(cluster.get("size"), len(members))
        # A confidence that is absent, null or not a number prints as "?"
        # rather than as a number the payload never carried.
        confidence = cluster.get("confidence")
        confidence_txt = "?"
        if isinstance(confidence, (int, float, str)):
            try:
                confidence_txt = f"{float(confidence):.2f}"
            except ValueError:
                confidence_txt = "?"
        sample = members[:sample_cap]
        more = size - len(sample)
        tail = f", +{more} more" if more > 0 else ""
        console_.print(
            f"  {confidence_txt}  {size} symbols: {', '.join(sample)}{tail}"
        )
    if truncated:
        console_.print(
            f"  … {truncated} further component(s) were found and not listed."
        )


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
    """Full dead-code report with confidence tiers, reasons, and abandoned cycles.

    Two populations, and the second is the one a one-hop check cannot find:
    single symbols nothing calls, and whole components that call only each other
    and that nothing outside reaches. Every member of such a component has an
    inbound edge, so it never appears in the first list at any confidence.
    """
    from collections import Counter

    from devcouncil.indexing.graph.liveness import confidence_at_least, confidence_label

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
        clusters = rust_dead.get("dead_clusters")
        clusters_truncated = _whole_number(rust_dead.get("dead_clusters_truncated"), 0)
        clusters_incomplete = rust_dead.get("dead_clusters_incomplete")
        # Skip Python graph load on successful Rust path.
        graph = None
    else:
        graph = _require_graph(root, warn_stale=False)
        entries = list(graph.dead_code)
        # `None` survives as `None`: a graph written before the component pass
        # existed did not run it, and copying that into an empty list would say
        # it ran and found nothing.
        clusters = None if graph.dead_clusters is None else list(graph.dead_clusters)
        clusters_truncated = graph.dead_clusters_truncated
        clusters_incomplete = graph.dead_clusters_incomplete
    if confidence:
        entries = [e for e in entries if confidence_label(e.confidence) == confidence]
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
                    # `None` and `[]` are different answers and both are kept:
                    # "the pass ran and found none" is a finding, "this
                    # generation predates the pass" is not.
                    "dead_clusters": clusters,
                    "dead_clusters_truncated": clusters_truncated,
                    "dead_clusters_incomplete": clusters_incomplete,
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
        # Printed even with no single-symbol findings, and especially then: an
        # empty `dead_code` list beside an unreported cluster is the shape that
        # says "nothing to clean up" about a repository with an abandoned
        # subsystem in it.
        _render_dead_clusters(console, clusters, clusters_truncated, clusters_incomplete)
        if stale and not allow_stale:
            status.print(
                "[red]refusing to treat a stale dead-code report as evidence "
                "(index built from a different commit than HEAD). Run `dev map` "
                "to refresh, or pass --allow-stale to accept.[/red]"
            )
            raise typer.Exit(code=3)
        return
    for e in entries:
        conf = confidence_label(e.confidence)
        console.print(
            f"{e.path}:{e.line}  {e.id}  [{conf}/{e.kind}]  {e.reason}"
        )
    reason_counts = Counter(e.reason or "(none)" for e in entries)
    console.print("")
    console.print("Reason summary:")
    for reason, n in reason_counts.most_common():
        console.print(f"  {n:4d}  {reason}")
    _render_dead_clusters(console, clusters, clusters_truncated, clusters_incomplete)
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
    # Raw on purpose, both loops: ``degree`` is a ``Counter`` count and
    # ``nodes`` a sorted list of edge endpoints, each derived by ``graph_check``
    # from fields ``CodeGraph`` typed at load. See the note above
    # ``_string_members``.
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
    # Raw on purpose: ``steps`` are ids ``CodeGraph`` typed at load, seeded
    # from ``entry_roots`` (``List[str]``) or the ``entry`` the caller typed.
    # See the note above ``_string_members``.
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
    """Diff / path blast radius: inbound callers, banded by distance.

    `--diff` names the seed set from the working tree; the walk itself is the
    kernel's, per path, and so are the bands. There is no Python engine below
    this: what used to sit here loaded the whole graph out of `index.sqlite`
    and re-ran the walk in Python, and it was reached not only when the kernel
    was absent but whenever a path had no indexed inbound edge — that is, every
    time the kernel correctly answered "nothing depends on this".
    """
    root = _root(project_root)
    if not diff and not paths:
        status.print("[red]Provide paths or --diff.[/red]")
        raise typer.Exit(code=1)
    if diff:
        from devcouncil.indexing.graph.intel import working_tree_changed_paths

        # Reads `git diff`, not the graph. When the caller also named paths,
        # they narrow the diff rather than replace it, as they always did.
        changed = working_tree_changed_paths(root)
        if paths:
            wanted = {str(p).replace("\\", "/") for p in paths}
            changed = [p for p in changed if p in wanted]
        seeds = changed
    else:
        seeds = [str(p) for p in (paths or [])]
    result = _devmap_query_payload(root, "impact", paths=seeds, max_depth=max_depth)
    if result is None:
        status.print(_NO_KERNEL_MESSAGE)
        raise typer.Exit(code=1)
    if result.get("ok") is False:
        status.print(f"[red]devmap: {result.get('error') or 'refused'}[/red]")
        raise typer.Exit(code=1)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if not result.get("paths"):
        console.print(
            "No paths to analyse." if diff else "No impacted paths."
        )
        return
    for item in result["paths"]:
        console.print(f"[bold]{item['path']}[/bold]")
        unavailable = item.get("unavailable")
        if unavailable:
            # Printed instead of an empty radius: "nothing looked" and "nothing
            # found" are the two readings this line exists to separate.
            console.print(f"  [yellow]{unavailable}[/yellow]")
        syms = item.get("symbols") or []
        if syms:
            label = "walk seeds" if item.get("symbols_are_walk_seeds") else "symbols"
            console.print(f"  {label}: " + ", ".join(s["id"] for s in syms[:8]))
        blast = item.get("blast") or {}
        for layer in blast.get("layers") or []:
            nodes = layer.get("nodes") or []
            # `confidence` is now the weakest edge that reached the band, and
            # `None` when the band holds no measured edge — printed as "-"
            # rather than as a tier name nothing measured.
            #
            # Round brackets, not square. This line read `[{confidence}]` and
            # Rich took `[extracted]` for console markup and *dropped it*: the
            # tier has never actually appeared in the output, for any band, on
            # either engine.
            tier = layer.get("confidence") or "-"
            omitted = layer.get("nodes_omitted") or 0
            console.print(
                f"  depth {layer['depth']} ({tier}): "
                f"{layer.get('count', len(nodes))} — " + ", ".join(nodes[:6])
                + (" …" if len(nodes) > 6 else "")
                + (f"  ({omitted} not listed)" if omitted else "")
            )
        if blast.get("layers_truncated"):
            console.print(
                f"  showing {blast.get('layers_shown')} of "
                f"{blast.get('layers_total')} bands (token budget)"
            )
        if blast.get("walk_incomplete"):
            console.print(f"  [yellow]warning: {blast['walk_incomplete']}[/yellow]")


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
    from devcouncil.indexing.graph.export import write_code_graph_okf

    root = _root(project_root)
    fmt = format.lower().strip()
    if fmt == "graphml":
        # The kernel is the only GraphML exporter (see
        # `devmap_engine.export_graphml`); no Python graph is loaded for it.
        from devcouncil.devmap_engine import DevMapEngineError, export_graphml

        try:
            if str(output) == "-":
                typer.echo(export_graphml(root)["text"], nl=False)
            else:
                out = output if output.is_absolute() else root / output
                out.parent.mkdir(parents=True, exist_ok=True)
                report = export_graphml(root, output=out)
                status.print(
                    f"[green]Wrote {out}[/green]  {report.get('nodes')} nodes, "
                    f"{report.get('edges')} edges; {report.get('edges_dangling')} "
                    f"edge(s) omitted (endpoint not a declared node), "
                    f"{report.get('characters_replaced')} character(s) replaced"
                )
        except DevMapEngineError as exc:
            status.print(f"[red]{exc}[/red]")
            raise typer.Exit(code=1) from exc
        return
    graph = _require_graph(root)
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
    root = _root(project_root)
    result = _require_kernel(root).routes()
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
            # Raw on purpose: ``handlers`` are ``_summarize_handler`` rows keyed
            # by ids ``CodeGraph`` typed at load. See the note above
            # ``_string_members``.
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
    root = _root(project_root)
    result = _require_kernel(root).shape_check(route_filter=route)
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
    root = _root(project_root)
    result = _require_kernel(root).api_impact(route_or_path)
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
    """Build or refresh the PDG layer for Python files.

    The layer lands in `.devcouncil/graph/pdg.json`. It used to be merged into
    the `CodeGraph` and written back with `write_code_graph`, which rewrites
    `code_graph.json` — the artifact the kernel is the only writer of — and
    persists the whole graph into the Python store on the way. Nothing outside
    these PDG commands ever read it back, and the next `dev map` run overwrote
    it anyway.
    """
    from devcouncil.indexing.graph.build import (
        build_pdg_for_paths,
        python_paths_for_pdg,
        write_pdg_layer,
    )

    root = _root(project_root)
    wanted = list(paths or [])
    if not wanted:
        wanted = python_paths_for_pdg(root)
        if not wanted:
            status.print(
                "[red]No file inventory at .devcouncil/repo_map.json; "
                "run `dev map` first.[/red]"
            )
            raise typer.Exit(code=1)
    layer = build_pdg_for_paths(root, paths=wanted)
    out = write_pdg_layer(root, layer)
    stats = (layer.to_meta() or {}).get("stats") or {}
    payload = {
        "ok": True,
        "stats": stats,
        "files": sorted(layer.files.keys()),
        "artifact": str(out.relative_to(root)) if out.is_relative_to(root) else str(out),
    }
    if json_output:
        typer.echo(json.dumps(payload, indent=2))
        return
    console.print(
        f"PDG: {stats.get('function_count', 0)} functions, "
        f"{stats.get('taint_count', 0)} taint findings across {stats.get('file_count', 0)} files"
    )
    console.print(f"Wrote {payload['artifact']}")


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
