"""Read-only MCP tools for querying ``.devcouncil/repo_map.json``.

Why ``_body`` runs in a thread
------------------------------

Every handler here splits into a synchronous ``_body`` and an ``async def
_run`` that does nothing but ``asyncio.to_thread(_body)``. The bodies are
blocking end to end — repo-map reads and content fingerprints, devmap IPC (a
``subprocess.run(timeout=10)`` binary probe, a daemon-readiness loop calling
``time.sleep(0.05)``, a 30 s socket deadline and a ``subprocess.run(timeout=120)``
CLI fallback), SQLite graph loads that re-materialise the whole graph, and LSP
sessions that spawn a language server. Run inline they park the asyncio event
loop for all of it, which is the failure ``util.run_cli_command``'s docstring
already spells out: while the loop is parked the server stops answering ``ping``
and cannot even receive the ``notifications/cancelled`` a client sends to give
up. That reasoning had been applied to the CLI helper alone.

The split changes only *where* the work runs. Nothing about what these handlers
answer depends on the thread it is computed on.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any, NamedTuple

from mcp.types import TextContent

from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.indexing.subsystem_map import (
    area_for_path,
    can_rule_out_adjacency,
    cross_boundary_pairs,
    handoff_paths_established,
    neighbors_established,
    role_files_established,
    dead_symbol_candidates_of,
    dependents_total_of,
    impact_targets,
    is_entry_root,
    unreachable_of,
    unwired_candidates_of,
)
from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    optional_bool_argument,
    optional_string_argument,
    optional_string_list_argument,
    with_codeintel_freshness,
)
from devcouncil.utils.json_persist import read_json

from .epistemic import with_epistemic


def _load_repo_map(root: Path) -> dict[str, Any] | None:
    map_path = root / ".devcouncil" / "repo_map.json"
    if not map_path.exists():
        return None
    try:
        data = read_json(map_path)
        return data if isinstance(data, dict) else None
    except Exception:
        return None


def _map_stale(root: Path, data: dict[str, Any] | None) -> bool:
    if not data:
        return False
    try:
        from devcouncil.devmap_client import DevMapClientError, try_connect

        client = try_connect(root)
        if client is not None:
            try:
                return client.is_map_stale()
            except DevMapClientError:
                pass
    except Exception:
        pass
    try:
        return RepoMapper(root).map_is_stale(data)
    except Exception:
        # Fail closed: unverifiable map must not be treated as fresh.
        return True


#: How many rows a single map response will carry. Every surface that applies
#: it reports `shown`/`total`/`truncated` so the cap is visible in the payload.
_ROW_LIMIT = 200

#: How many paths one `devcouncil_impact` call will analyse.
#:
#: Each path costs a kernel `impact` round trip (or an LSP reference query), so
#: an unbounded array was an unbounded amount of blocking work with no timeout,
#: no cap and no partial result: 10 000 paths meant 10 000 sequential IPC
#: exchanges. The schema advertises the same bound, and the response reports
#: `paths_requested`/`paths_analyzed`/`paths_truncated` so a caller is never
#: shown a capped answer that looks complete.
_MAX_IMPACT_PATHS = 100


def _graph_degraded_fields(root: Path) -> dict[str, Any]:
    """Surface lean-map handshake on legacy graph tool responses.

    ``graph_degraded`` is tri-state. With no repo map there is nothing to read
    the flag from, and the old ``bool(None)`` published a confident "the graph
    is fine" for a repository that has never been mapped.
    """
    data = _load_repo_map(root)
    if data is None:
        return {
            "graph_degraded": None,
            "graph_degraded_reason": (
                "no repo map at .devcouncil/repo_map.json, so lean-map status is "
                "unknown; run `dev map`"
            ),
        }
    degraded = bool(data.get("graph_degraded"))
    return {
        "graph_degraded": degraded,
        "graph_degraded_reason": (
            str(data.get("graph_degraded_reason") or "") if degraded else ""
        ),
    }


def _graph_payload(root: Path, result: dict[str, Any]) -> dict[str, Any]:
    """Build a graph-tool response whose ``ok`` is *derived*, never asserted.

    The Python graph surfaces answered a failure with
    ``{"error": "no code graph; run `dev map` first"}``. Prepending a literal
    ``"ok": True`` published that failure as a success, so a caller branching on
    ``ok`` read "the graph has no callers for this symbol" from a response that
    means "there is no graph".

    Those surfaces are retired — the two remaining producers are the kernel
    route commands (via :func:`_route_tool`) and `diff_impact` — but the rule
    stays here rather than at each call site: a producer that grows an ``error``
    key must not have to remember to flip ``ok`` too.
    """
    error = result.get("error")
    payload: dict[str, Any] = {"ok": not error, **result, **_graph_degraded_fields(root)}
    if error:
        payload["ok"] = False
        payload.setdefault("code", "graph_unavailable")
    return payload


def _subsystem_summary(sub: dict[str, Any]) -> dict[str, Any]:
    return {
        "area": sub.get("area"),
        "summary": sub.get("summary") or "",
    }


def _subsystem_detail(
    sub: dict[str, Any],
    *,
    neighbors_computed: bool = False,
    handoff_paths_computed: bool = False,
    role_files_computed: bool = False,
) -> dict[str, Any]:
    """One subsystem's detail rows, with the derived fields carrying whether they are answers.

    ``neighbors_computed`` and ``handoff_paths_computed`` default to False
    because that is the fail-closed reading: a caller that did not pass the map
    cannot vouch for either field, and an empty list from the kernel means "not
    computed" far more often than "none". They are two flags rather than one
    because the kernel derived the two fields in two separate passes of its
    history, so a map can genuinely carry one and not the other.
    """
    role_files = sub.get("role_files") or {}
    if not isinstance(role_files, dict):
        role_files = {}
    return {
        "area": sub.get("area"),
        "summary": sub.get("summary") or "",
        "entry_points": list(sub.get("entry_points") or []),
        "critical_files": list(sub.get("critical_files") or []),
        "neighbors": list(sub.get("neighbors") or []),
        "neighbors_computed": neighbors_computed,
        "handoff_paths": list(sub.get("handoff_paths") or []),
        "handoff_paths_computed": handoff_paths_computed,
        "role_files": {str(k): list(v or []) for k, v in role_files.items()},
        "role_files_computed": role_files_computed,
        # The real per-role totals. `role_files` is a capped sample, so without
        # these a caller reads four names as the whole bucket.
        "role_file_counts": {
            str(k): v
            for k, v in (sub.get("role_file_counts") or {}).items()
            if isinstance(v, int) and not isinstance(v, bool)
        },
    }


def _find_subsystem(data: dict[str, Any], area: str) -> dict[str, Any] | None:
    for sub in data.get("subsystems") or []:
        if isinstance(sub, dict) and str(sub.get("area") or "") == area:
            return sub
    return None


def _path_from_dead_symbol(entry: str) -> str:
    """Extract the file path from a ``path:line name`` dead-symbol entry."""
    loc = entry.split(" ", 1)[0]
    path, _, _ = loc.rpartition(":")
    return path.replace("\\", "/")


def _matches_filters(
    path: str,
    data: dict[str, Any],
    *,
    area: str | None,
    path_prefix: str | None,
) -> bool:
    norm = path.replace("\\", "/")
    if path_prefix:
        prefix = path_prefix.replace("\\", "/").rstrip("/")
        if not (norm == prefix or norm.startswith(prefix + "/")):
            return False
    if area:
        return area_for_path(norm, data) == area
    return True


def _filter_paths(
    paths: list[str],
    data: dict[str, Any],
    *,
    area: str | None,
    path_prefix: str | None,
) -> list[str]:
    if not area and not path_prefix:
        return paths
    return [p for p in paths if _matches_filters(p, data, area=area, path_prefix=path_prefix)]


def _filter_dead_symbols(
    entries: list[str],
    data: dict[str, Any],
    *,
    area: str | None,
    path_prefix: str | None,
) -> list[str]:
    if not area and not path_prefix:
        return entries
    return [
        e
        for e in entries
        if _matches_filters(_path_from_dead_symbol(e), data, area=area, path_prefix=path_prefix)
    ]


async def handle_repo_map(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        subsystem = optional_string_argument(arguments, "subsystem")
        path = optional_string_argument(arguments, "path")
        for arg_name, value in [("subsystem", subsystem), ("path", path)]:
            if value == "":
                return error_text(
                    f"{arg_name} must be a string",
                    code="invalid_arguments",
                    argument=arg_name,
                )

        data = _load_repo_map(root)
        if data is None:
            return error_text(
                "No repo map found. Run `dev map` to generate .devcouncil/repo_map.json.",
                code="map_missing",
            )

        stale = _map_stale(root, data)

        # Computed once and attached to every success return below.
        #
        # These fields used to appear on exactly one of the three: a `path`
        # whose area resolved to a known subsystem took the subsystem branch and
        # came back with no symbol listing and nothing saying one had not been
        # attempted — so the better-mapped a file was, the less `repo_map` told
        # you about it. And the no-path branch carried a bare `symbols: []`,
        # which reads as "no symbols here" rather than "you did not ask".
        # A consumer indexing `symbols_available` got a KeyError on two of the
        # three branches.
        symbol_fields = _symbol_fields(root, path)
        resolved_area: str | None = None
        if path:
            resolved_area = area_for_path(path, data)
            if not subsystem and resolved_area:
                subsystem = resolved_area

        if subsystem:
            sub = _find_subsystem(data, subsystem)
            if sub is None:
                return json_text({
                    "ok": True,
                    "stale": stale,
                    "subsystem": None,
                    "path": path,
                    "area": resolved_area,
                    "error": f"Unknown subsystem area: {subsystem}",
                    "code": "unknown_subsystem",
                    **symbol_fields,
                })
            return json_text({
                "ok": True,
                "stale": stale,
                "path": path,
                "area": resolved_area or subsystem,
                "subsystem": _subsystem_detail(
                    sub,
                    neighbors_computed=neighbors_established(data),
                    handoff_paths_computed=handoff_paths_established(data),
                    role_files_computed=role_files_established(data),
                ),
                **symbol_fields,
            })

        subsystems = [
            _subsystem_summary(s)
            for s in (data.get("subsystems") or [])
            if isinstance(s, dict) and s.get("area")
        ]
        payload: dict[str, Any] = {
            "ok": True,
            "stale": stale,
            "languages": list(data.get("languages") or []),
            "frameworks": list(data.get("frameworks") or []),
            "package_managers": list(data.get("package_managers") or []),
            "subsystems": subsystems,
            "path": path,
            "area": resolved_area,
            **symbol_fields,
        }
        return json_text(payload)

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


def _symbol_fields(root: Path, path: str | None) -> dict[str, Any]:
    """The symbol listing for `path`, in the one shape every branch publishes.

    `symbols_available` is the discriminator: ``True``/``False`` is a determined
    answer, and ``None`` means the question was never put — no path was asked
    about. A caller must never have to test whether the key is present to find
    out which of the three it got, and ``[]`` must never stand in for "not
    determined".
    """
    if not path:
        return {
            "symbols": [],
            "symbols_available": None,
            "symbols_source": None,
            "symbols_shown": 0,
            "symbols_total": None,
            "symbols_scan_total": None,
            "symbols_truncated": False,
            "symbols_walk_incomplete": None,
            "symbols_reason": "not requested: no path argument",
        }
    symbols = _symbols_for_path(root, path)
    fields: dict[str, Any] = {
        "symbols": symbols.items,
        "symbols_available": symbols.ok,
        "symbols_source": symbols.source,
        "symbols_shown": len(symbols.items),
        "symbols_total": symbols.total if symbols.ok else None,
        # The producer's own count, carried beside the local one so a capped
        # scan is never presented as complete coverage.
        "symbols_scan_total": symbols.scan_total if symbols.ok else None,
        "symbols_truncated": symbols.truncated,
        "symbols_walk_incomplete": symbols.walk_incomplete,
    }
    if not symbols.ok:
        fields["symbols_reason"] = symbols.reason
    return fields


class SymbolScan(NamedTuple):
    """A per-path symbol listing plus whether the listing could be produced.

    ``ok=False`` means no engine answered — never "this file has no symbols".
    The two used to be the same empty list.

    ``total`` is the number of symbols found for the path, and it is ``None``
    when the *producer* truncated: a total counted from the rows that survived a
    capped scan is not a total, and publishing one is how a 5000-candidate store
    reported three. ``scan_total`` carries the producer's own count beside it so
    both numbers travel together rather than one standing in for the other.
    """

    ok: bool
    source: str
    reason: str
    items: list[dict[str, Any]]
    total: int | None
    truncated: bool
    scan_total: int | None = None
    walk_incomplete: str | None = None


def _scan_ok(
    items: list[dict[str, Any]],
    source: str,
    *,
    producer_total: int | None = None,
    producer_truncated: bool = False,
    walk_incomplete: str | None = None,
) -> SymbolScan:
    """Build a successful scan, keeping the producer's own counts intact.

    ``truncated`` is the *union* of the two things that withhold rows: this
    response's row cap and whatever the engine upstream already withheld. It
    used to be the row cap alone, so a kernel that answered
    ``total=5000, truncated=True`` was republished as ``truncated=False`` —
    a caller was told it had been shown everything by a response that had been
    shown 3 of 5000.
    """
    shown = items[:_ROW_LIMIT]
    row_capped = len(shown) < len(items)
    return SymbolScan(
        ok=True,
        source=source,
        reason="",
        items=shown,
        total=None if producer_truncated else len(items),
        truncated=row_capped or producer_truncated,
        scan_total=producer_total,
        walk_incomplete=walk_incomplete,
    )


def _symbols_for_path(root: Path, path: str) -> SymbolScan:
    """Per-path symbol listings from the kernel, or an explicit failure.

    Every reason the kernel declined is kept and reported: a missing kernel, a
    locked store, a mid-build truncation and a file that genuinely defines
    nothing all used to return the same ``[]``.

    There is no second engine. A `load_code_graph` fallback used to sit below
    this — the retired Python engine's whole-graph read, 855 ms and ~690 MB RSS
    on this repository, 3.8 s and a 102 MB `index.sqlite` write on the first
    call — and it published through :func:`_scan_ok` with no producer total and
    no truncation flag, so a fallback answer reached the agent wearing the shape
    of a complete, verified one. A check that could not run must not report what
    a check that ran and passed reports; the kernel being unreachable is
    reported as that.
    """
    norm = path.replace("\\", "/")
    reasons: list[str] = []
    try:
        from devcouncil.devmap_client import (
            DevMapClientError,
            resolution_unavailable_reason,
            try_connect,
            walk_incomplete_reason,
        )

        client = try_connect(root)
        if client is None:
            reasons.append("no built devmap store (run `dev map`)")
        else:
            resp = client.search(norm, limit=2000)
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                raise DevMapClientError(reason)
            if resp.total > 0 and not resp.items and resp.truncated:
                raise DevMapClientError(
                    f"devmap search returned truncated empty items (total={resp.total})"
                )
            incomplete = walk_incomplete_reason(resp)
            out: list[dict[str, Any]] = []
            for item in resp.items:
                file_path = str(item.get("file_path") or "").replace("\\", "/")
                if file_path != norm and not file_path.endswith("/" + norm):
                    # Keep path-prefix hits from search when exact path misses.
                    if norm not in file_path:
                        continue
                name = str(item.get("symbol_name") or "")
                span = item.get("span") or (0, 0)
                line = int(span[0]) if isinstance(span, (list, tuple)) and span else 0
                kind = str(item.get("kind") or "symbol")
                if kind == "file":
                    continue
                out.append({
                    "id": f"{file_path}::{name}" if file_path and name else name or file_path,
                    "kind": kind,
                    "name": name,
                    "line": line,
                })
            if incomplete and not out:
                # "I stopped looking" published as "this file defines nothing"
                # is the reading that gets a live symbol deleted, so it fails
                # closed to the same unavailable answer a raising engine gets.
                raise DevMapClientError(f"devmap search walk incomplete: {incomplete}")
            return _scan_ok(
                out,
                "devmap",
                producer_total=resp.total,
                producer_truncated=bool(resp.truncated),
                walk_incomplete=incomplete,
            )
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        reasons.append(f"devmap: {exc}")
    return SymbolScan(False, "", "; ".join(reasons), [], 0, False)


async def handle_impact(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        paths, list_error = optional_string_list_argument(arguments, "paths")
        if list_error:
            return list_error
        if not paths:
            return error_text("Missing paths", code="missing_argument", argument="paths")
        precise, precise_error = optional_bool_argument(arguments, "precise")
        if precise_error:
            return precise_error
        precise = bool(precise)

        data = _load_repo_map(root)
        if data is None:
            return error_text(
                "No repo map found. Run `dev map` to generate .devcouncil/repo_map.json.",
                code="map_missing",
            )

        stale = _map_stale(root, data)
        # Bounded before any per-path work starts. `requested` and `analyzed`
        # both ride on the response, so a capped answer is never mistaken for
        # coverage of everything the caller asked about.
        requested = len(paths)
        analyzed_paths = paths[:_MAX_IMPACT_PATHS]
        lsp_pool = None
        lsp_pool_error = ""
        if precise:
            try:
                from devcouncil.indexing.lsp_client import LspSessionPool

                lsp_pool = LspSessionPool(root)
            except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                lsp_pool = None
                lsp_pool_error = str(exc)

        # Connected once, not once per path. `try_connect` probes the binary
        # with `subprocess.run(timeout=10)` and may spawn and wait on the
        # daemon, so doing it inside the loop doubled the IPC round trips per
        # element for an answer that cannot change between iterations.
        client = None
        connect_error = ""
        if not precise:
            try:
                from devcouncil.devmap_client import try_connect

                client = try_connect(root)
            except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                connect_error = str(exc)

        try:
            items: list[dict[str, Any]] = []
            all_neighbor_areas: set[str] = set()
            for raw in analyzed_paths:
                path = raw.replace("\\", "/")
                dependents, neighbors = impact_targets(path, data)
                # `import` is the repo-map heuristic — the weakest of the three.
                # Which engine actually answered rides on every item now: it used
                # to be emitted only under `precise`, so a kernel failure left
                # heuristic dependents looking like a symbol-level answer.
                resolution = "import"
                resolution_reason = ""
                walk_note: str | None = None
                fallback = "; dependents come from the repo-map import heuristic"
                if not precise:
                    try:
                        from devcouncil.devmap_client import (
                            DevMapClientError,
                            resolution_unavailable_reason,
                            walk_incomplete_reason,
                        )

                        if client is None:
                            detail = (
                                f"devmap unavailable: {connect_error}"
                                if connect_error
                                else "no built devmap store (run `dev map`)"
                            )
                            resolution_reason = detail + fallback
                        else:
                            resp = client.impact(path, depth=1)
                            reason = resolution_unavailable_reason(resp.resolution)
                            if reason:
                                raise DevMapClientError(reason)
                            walk_note = walk_incomplete_reason(resp)
                            rust_deps = sorted({
                                str(edge.get("source_file") or "")
                                for edge in resp.items
                                if edge.get("source_file")
                            })
                            if rust_deps:
                                dependents = rust_deps
                                resolution = "devmap"
                            elif walk_note:
                                # No edges from a walk that stopped early is not
                                # "nothing imports this file" — it is "the walk
                                # did not finish", and the two must not reach a
                                # caller wearing the same shape.
                                resolution_reason = (
                                    f"devmap walk incomplete: {walk_note}" + fallback
                                )
                            else:
                                resolution_reason = (
                                    "devmap reported no dependent edges" + fallback
                                )
                    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                        resolution_reason = f"devmap unavailable: {exc}{fallback}"
                elif lsp_pool is None:
                    detail = f": {lsp_pool_error}" if lsp_pool_error else ""
                    resolution_reason = f"LSP session pool unavailable{detail}{fallback}"
                else:
                    try:
                        lsp_deps = lsp_pool.dependents_of_file(path)
                        if lsp_deps is not None:
                            dependents = lsp_deps
                            resolution = "lsp"
                        else:
                            resolution_reason = (
                                "LSP returned no dependents for this file" + fallback
                            )
                    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                        resolution_reason = f"LSP dependents failed: {exc}{fallback}"
                area = area_for_path(path, data)
                all_neighbor_areas.update(neighbors)
                item: dict[str, Any] = {
                    "path": path,
                    "area": area,
                    "is_entry_root": is_entry_root(path, data),
                    "dependents": dependents,
                    "neighbors": neighbors,
                    "resolution": resolution,
                    "resolution_reason": resolution_reason,
                    # Present on every item, `None` when the walk finished, so
                    # a caller reads one key instead of testing for its
                    # existence. A non-empty `dependents` keeps its rows and
                    # still says the walk was capped.
                    "walk_incomplete": walk_note,
                }
                # `dependents_total` counts the repo map's *import* universe. It
                # is a valid "shown of total" only while `dependents` is that
                # same list; pairing it with a devmap or LSP answer would report
                # a total for a set the response does not contain.
                if resolution == "import":
                    dep_total = dependents_total_of(path, data)
                    if dep_total is not None and dep_total > len(dependents):
                        item["dependents_total"] = dep_total
                items.append(item)

            crossings = [
                {"areas": [a, b]}
                for a, b in cross_boundary_pairs(analyzed_paths, data)
            ]
            # An empty `cross_boundary_pairs` means one of two different things,
            # and a client cannot tell them apart from the list alone: every
            # touched pair is adjacent, or this map cannot rule adjacency out —
            # it never derived the relation, or holds only a capped part of it.
            # Same rule as `is_entry_root` and `walk_incomplete` above: the
            # answer carries whether it could be given.
            crossings_checked = can_rule_out_adjacency(data)
            payload: dict[str, Any] = {
                "ok": True,
                "stale": stale,
                "paths": items,
                "paths_requested": requested,
                "paths_analyzed": len(analyzed_paths),
                "paths_truncated": len(analyzed_paths) < requested,
                "neighbor_areas": sorted(all_neighbor_areas),
                "cross_boundary_pairs": crossings,
                "cross_boundary_checked": crossings_checked,
            }
            if precise:
                payload["precise"] = True
            return json_text(payload)
        finally:
            if lsp_pool is not None:
                lsp_pool.close()

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_liveness(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        area = optional_string_argument(arguments, "area")
        path_prefix = optional_string_argument(arguments, "path_prefix")
        min_confidence = optional_string_argument(arguments, "min_confidence") or "inferred"
        for arg_name, value in [
            ("area", area),
            ("path_prefix", path_prefix),
            ("min_confidence", min_confidence),
        ]:
            if value == "":
                return error_text(
                    f"{arg_name} must be a string",
                    code="invalid_arguments",
                    argument=arg_name,
                )
        if min_confidence not in {"extracted", "inferred", "ambiguous"}:
            return error_text(
                "min_confidence must be extracted|inferred|ambiguous",
                code="invalid_arguments",
                argument="min_confidence",
            )

        data = _load_repo_map(root)
        if data is None:
            return error_text(
                "No repo map found. Run `dev map` to generate .devcouncil/repo_map.json.",
                code="map_missing",
            )

        stale = _map_stale(root, data)
        raw_roots = data.get("entry_roots") or []
        if not isinstance(raw_roots, list):
            raw_roots = []
        global_entry_roots = [str(p) for p in raw_roots]

        unwired = _filter_paths(
            unwired_candidates_of(data), data, area=area, path_prefix=path_prefix
        )
        unreachable = _filter_paths(
            unreachable_of(data), data, area=area, path_prefix=path_prefix
        )
        dead_symbols = _filter_dead_symbols(
            dead_symbol_candidates_of(data), data, area=area, path_prefix=path_prefix
        )
        entry_roots = global_entry_roots
        if area or path_prefix:
            entry_roots = _filter_paths(entry_roots, data, area=area, path_prefix=path_prefix)

        dead_scan = _structured_dead_code(
            root,
            area=area,
            path_prefix=path_prefix,
            min_confidence=min_confidence,
        )
        # Reliability is a GLOBAL property of the map: an area/path_prefix scope that
        # merely contains no entry root (normal for library subsystems) must not
        # flip the response to unreliable and drop its unreachable list.
        unreachable_unreliable = (
            data.get("liveness_unreachable_unreliable") is True
            or not global_entry_roots
        )
        warn = None
        if unreachable_unreliable:
            unreachable = []
            warn = (
                "entry_roots empty or liveness_unreachable_unreliable: "
                "unreachable_files omitted; ignore mass inferred dead"
            )
        if data.get("graph_degraded"):
            warn = (
                (warn + "; " if warn else "")
                + "graph_degraded: lean map — treat liveness/dead tiers as unreliable"
            )
        if not dead_scan.ok:
            # An empty `dead_code` from a scan that never ran used to be
            # indistinguishable from a clean repository. It says so now, and it
            # says so in the field callers already read for reliability.
            warn = (
                (warn + "; " if warn else "")
                + f"dead-code scan unavailable: {dead_scan.reason}"
            )
        payload: dict[str, object] = {
            "ok": True,
            "stale": stale,
            "graph_degraded": bool(data.get("graph_degraded")),
            "area": area,
            "path_prefix": path_prefix,
            "min_confidence": min_confidence,
            "entry_roots": entry_roots,
            "unwired_candidates": unwired,
            "unreachable_files": unreachable,
            "dead_symbol_candidates": dead_symbols,
            "dead_code": dead_scan.items,
            "dead_code_available": dead_scan.ok,
            "dead_code_source": dead_scan.source,
            "dead_code_shown": len(dead_scan.items),
            "dead_code_total": dead_scan.total if dead_scan.ok else None,
            # The producing engine's own candidate count, carried beside the
            # scoped one: when the producer truncated, `dead_code_total` is
            # None (unknowable from a capped sample) and this is what the
            # producer actually saw.
            "dead_code_scan_total": dead_scan.scan_total if dead_scan.ok else None,
            "dead_code_truncated": dead_scan.truncated,
            "dead_code_walk_incomplete": dead_scan.walk_incomplete,
            # Every in-scope candidate this response does not carry: the
            # confidence filter *and* the row cap. It used to count only the
            # filter, so 500 candidates reported 200 items and 0 hidden.
            "dead_code_hidden": dead_scan.hidden if dead_scan.ok else None,
            "dead_code_hidden_low_confidence": (
                dead_scan.hidden_low_confidence if dead_scan.ok else None
            ),
            "unreachable_unreliable": unreachable_unreliable
            or bool(data.get("graph_degraded")),
        }
        if not dead_scan.ok:
            payload["dead_code_reason"] = dead_scan.reason
        if warn:
            payload["warning"] = warn
        # W2.2: the structured form of the caveat this tool's description used to
        # carry as prose ("if entry_roots are empty or unreachable_unreliable is
        # true, ignore unreachable_files"). Every boundary is something the map
        # already recorded; the field only stops a caller from having to remember
        # the rule and recombine the keys themselves.
        with_epistemic(
            payload,
            coverage_gaps=(data.get("liveness_meta") or {}).get("coverage_gaps"),
            degraded_reason=(
                "graph_degraded: the map is lean, so liveness and dead tiers are a lower bound"
                if data.get("graph_degraded")
                else None
            ),
            truncated=bool(dead_scan.ok and dead_scan.total and dead_scan.total > len(dead_scan.items)),
            shown=len(dead_scan.items),
            total=dead_scan.total if dead_scan.ok else None,
            extra_boundaries=[
                (
                    "unreachable_files omitted: entry roots are empty or the component pass "
                    "could not answer reachability"
                    if unreachable_unreliable
                    else ""
                ),
                (f"dead-code scan unavailable: {dead_scan.reason}" if not dead_scan.ok else ""),
                (
                    f"scoped to area={area!r} path_prefix={path_prefix!r}: findings outside "
                    "that scope are not counted"
                    if (area or path_prefix)
                    else ""
                ),
            ],
        )
        return json_text(payload)

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


class DeadCodeScan(NamedTuple):
    """A dead-code listing plus proof that the scan actually ran.

    ``ok=False`` means no engine produced a listing — a missing kernel, a
    locked store, a mid-build truncation or a raising graph load. It never
    means "this repository has no dead code", which is what the old
    ``([], 0)`` said in all four cases.

    ``total`` counts every candidate the scan saw within the requested scope,
    before the confidence filter and before the row cap, so a caller can always
    reconstruct what it is not being shown — and it is ``None`` when the
    *producer* truncated, because a scoped total counted from the rows that
    survived a capped scan is not a total. ``scan_total`` carries the producer's
    own count beside it, so both numbers travel together and a capped sample is
    never published as complete coverage.
    """

    ok: bool
    source: str
    reason: str
    items: list[dict[str, Any]]
    total: int | None
    hidden_low_confidence: int
    truncated: bool
    scan_total: int | None = None
    walk_incomplete: str | None = None

    @property
    def hidden(self) -> int | None:
        """Candidates in scope that this response does not carry, when known."""
        if self.total is None:
            return None
        return self.total - len(self.items)


def _dead_scan_ok(
    matched: list[dict[str, Any]],
    *,
    source: str,
    total: int,
    hidden_low_confidence: int,
    producer_total: int | None = None,
    producer_truncated: bool = False,
    walk_incomplete: str | None = None,
) -> DeadCodeScan:
    """Build a successful scan, keeping the producer's own counts intact.

    ``truncated`` is the union of the row cap here and whatever the engine
    upstream already withheld. Deriving it from the surviving rows alone is what
    let a kernel answer of ``total=5000, truncated=True, hidden=4997`` reach
    ``devcouncil_liveness`` as ``dead_code_total: 3, dead_code_truncated: false,
    dead_code_hidden: 0`` — a budget-capped scan reported as a clean repository.
    The sibling ``codeintel._dead_via_client`` always passed the kernel's own
    numbers through; the two now answer the same question the same way.
    """
    shown = matched[:_ROW_LIMIT]
    row_capped = len(shown) < len(matched)
    return DeadCodeScan(
        ok=True,
        source=source,
        reason="",
        items=shown,
        total=None if producer_truncated else total,
        hidden_low_confidence=hidden_low_confidence,
        truncated=row_capped or producer_truncated,
        scan_total=producer_total,
        walk_incomplete=walk_incomplete,
    )


def _structured_dead_code(
    root: Path,
    *,
    area: str | None,
    path_prefix: str | None,
    min_confidence: str = "inferred",
) -> DeadCodeScan:
    """Dead-symbol candidates from the kernel, or an explicit failure.

    The kernel is the only engine here, for the reason
    :func:`_symbols_for_path` states: the `load_code_graph` fallback that used
    to sit below published through :func:`_dead_scan_ok` with
    ``producer_total=None, producer_truncated=False``, so a Python answer
    reached `devcouncil_liveness` wearing the kernel's "complete and
    untruncated" shape.
    """
    data = _load_repo_map(root) or {}
    reasons: list[str] = []
    try:
        from devcouncil.devmap_client import (
            DevMapClientError,
            resolution_unavailable_reason,
            try_connect,
            walk_incomplete_reason,
        )
        from devcouncil.indexing.graph.liveness import confidence_at_least

        client = try_connect(root)
        if client is None:
            reasons.append("no built devmap store (run `dev map`)")
        else:
            resp = client.dead_symbols(budget=2000)
            reason = resolution_unavailable_reason(resp.resolution)
            if reason:
                raise DevMapClientError(reason)
            if resp.total > 0 and not resp.items and resp.truncated:
                raise DevMapClientError(
                    f"devmap dead_symbols returned truncated empty items (total={resp.total})"
                )
            incomplete = walk_incomplete_reason(resp)
            matched: list[dict[str, Any]] = []
            in_scope = 0
            hidden = 0
            for item in resp.items:
                path = str(item.get("file_path") or item.get("path") or "")
                if not _matches_filters(path, data, area=area, path_prefix=path_prefix):
                    continue
                in_scope += 1
                conf = item.get("confidence", "inferred")
                if isinstance(conf, (int, float)):
                    score = float(conf)
                    conf = (
                        "extracted" if score >= 0.9 else "inferred" if score >= 0.5 else "ambiguous"
                    )
                if not confidence_at_least(conf, min_confidence):
                    hidden += 1
                    continue
                name = str(item.get("symbol_name") or item.get("name") or "")
                span = item.get("span") or (0, 0)
                line = (
                    int(span[0])
                    if isinstance(span, (list, tuple)) and span
                    else int(item.get("line") or 0)
                )
                matched.append({
                    **item,
                    "id": str(
                        item.get("id")
                        or (f"{path}::{name}" if path and name else name or path)
                    ),
                    "path": path,
                    "name": name,
                    "line": line,
                    "confidence": str(conf),
                })
            if incomplete and not matched:
                raise DevMapClientError(f"devmap dead_symbols walk incomplete: {incomplete}")
            return _dead_scan_ok(
                matched,
                source="devmap",
                total=in_scope,
                hidden_low_confidence=hidden,
                producer_total=resp.total,
                producer_truncated=bool(resp.truncated),
                walk_incomplete=incomplete,
            )
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        reasons.append(f"devmap: {exc}")
    return DeadCodeScan(False, "", "; ".join(reasons), [], 0, 0, False)


async def handle_graph_ingest(root: Path, arguments: dict) -> list[TextContent]:
    """Rebuild the map through the Rust kernel — the same writer `dev map` uses.

    This used to run the Python engine into `index.sqlite` and rewrite
    `repo_map.json` from it. The SessionStart hook recommends this tool as the
    equivalent of `dev map`; it refreshed a store that `dev map query` does
    not read and overwrote the map the kernel had just written.
    """
    paths, _ = optional_string_list_argument(arguments, "paths")
    from devcouncil.devmap_engine import DevMapEngineError
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    changed = list(paths or [])
    map_path = root / ".devcouncil" / "repo_map.json"
    try:
        refresh = await asyncio.to_thread(refresh_map_artifacts, root, map_path, quiet=True)
    except DevMapEngineError as exc:
        # `code` stays the MCP-level class; the kernel's own diagnosis rides
        # along so the caller can branch on it and run the fix.
        return error_text(
            str(exc),
            code="engine_unavailable",
            paths=changed,
            kernel_code=exc.code,
            fix=exc.fix,
            run_id=exc.run_id,
            stage=exc.stage,
        )
    kernel = getattr(refresh, "kernel_status", None)
    payload: dict[str, Any] = {
        "ok": not refresh.degraded,
        "paths": changed,
        "generation": refresh.generation,
        "mode": refresh.mode,
        "degraded": refresh.degraded,
        "reason": refresh.reason,
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
        if not kernel.is_fresh or kernel.degraded_reason:
            payload["detail"] = (
                "The kernel store is not fresh after this build: "
                f"{kernel.degraded_reason or 'pending paths remain'}. Files it could not "
                "index are absent from the graph; `dev map repair --pending` drops stuck entries."
            )
    return json_text(payload)


async def handle_graph_doctor(root: Path, arguments: dict) -> list[TextContent]:
    """Coded checks with exact fixes; `fix: true` applies what can be applied."""
    import asyncio

    from devcouncil.devmap_health import apply_fixes, run_doctor

    if bool(arguments.get("fix")):
        result = await asyncio.to_thread(apply_fixes, root)
        return json_text({
            "ok": result["ok"],
            "applied": result["applied"],
            "not_applied": result["not_applied"],
            "checks": result["after"]["checks"],
            "build": result["after"]["status"]["build"],
        })
    result = await asyncio.to_thread(run_doctor, root)
    return json_text({
        "ok": result["ok"],
        "checks": result["checks"],
        "build": result["status"]["build"],
        "last_build": result["status"]["last_build"],
    })


async def handle_graph_runs(root: Path, arguments: dict) -> list[TextContent]:
    """Records of recent kernel runs from the project trace log."""
    import asyncio

    from devcouncil.devmap_engine import read_run_history
    from devcouncil.integrations.mcp.util import int_argument

    limit = int_argument(arguments, "limit", 10, minimum=1, maximum=200)
    failed_only = bool(arguments.get("failedOnly"))
    history = await asyncio.to_thread(
        read_run_history, root, limit=limit, failed_only=failed_only
    )
    # `runs: []` used to be the answer for three different things: no trace log,
    # a log whose every line was unreadable, and a kernel that has genuinely not
    # run. Only the last is what an empty list reads as, and a caller asking
    # "has this been built?" acted on the other two as if it had been answered.
    return json_text({
        "ok": True,
        "runs": history.runs,
        "log_present": history.log_present,
        "unparsed_lines": history.unparsed_lines,
        "shown": len(history.runs),
        "total": history.total,
        "truncated": history.truncated,
    })


async def handle_graph_cypher(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        query = optional_string_argument(arguments, "query")
        if not query:
            return error_text("Missing query", code="missing_argument", argument="query")
        from devcouncil.indexing.graph.cypher import run_cypher

        return json_text(run_cypher(root, query))

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_pdg_query(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        mode = optional_string_argument(arguments, "mode")
        target = optional_string_argument(arguments, "target")
        if not mode:
            return error_text("Missing mode", code="missing_argument", argument="mode")
        if not target:
            return error_text("Missing target", code="missing_argument", argument="target")
        variable = optional_string_argument(arguments, "variable")
        from devcouncil.indexing.graph.query import query_pdg_controls, query_pdg_flows

        if mode == "controls":
            return json_text(query_pdg_controls(root, target))
        if mode == "flows":
            return json_text(query_pdg_flows(root, target, variable=variable or None))
        return error_text(
            "mode must be controls or flows",
            code="invalid_arguments",
            argument="mode",
        )

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_explain(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        path = optional_string_argument(arguments, "path")
        category = optional_string_argument(arguments, "category")
        from devcouncil.indexing.graph.query import explain_pdg_taint

        return json_text(explain_pdg_taint(root, path=path or None, category=category or None))

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


def _devmap_query_payload(root: Path, kind: str, **kwargs):
    """Kernel-backed answer for a query surface, or ``None`` to fall back.

    Deliberately re-exported from the CLI rather than reimplemented: the
    composition (search, then bounded `impact`/`deps` per definition, with
    `importers` reported as *not computed* because devmap exposes no Imports
    edges) is already proven there, and a second copy is how the two answers
    start to differ. The import is lazy because it pulls the CLI module; the MCP
    server is long-lived, so that cost is paid once per process.

    Follow-up worth doing: move `_devmap_query_payload` to a module that neither
    the CLI nor MCP owns, and have both import it, rather than MCP reaching into
    `cli.commands`.
    """
    from devcouncil.cli.commands.graph_cmd import (
        _devmap_query_payload as _cli_devmap_query_payload,
    )

    return _cli_devmap_query_payload(root, kind, **kwargs)


async def handle_graph_query(root: Path, arguments: dict) -> list[TextContent]:
    """Symbol lookup. The kernel is the only engine; no kernel is an error.

    This answered entirely from Python: `query_symbol` -> `load_code_graph` ->
    `index.sqlite`, re-materialising 14,057 nodes and 71,195 edges as pydantic
    models on **every** call. Measured at 1.057 s against 0.154 s for the
    kernel-backed `devcouncil_code_search` answering the same class of question,
    with ~90% of it in that re-materialisation — and the first call after any
    kernel build additionally cost 6.5-7.5 s and wrote 242 MB of SQLite under a
    writer lease, from a tool an agent reads as read-only. `query_symbol` has
    since been deleted; the kernel-first shape became kernel-only.
    """

    def _body() -> list[TextContent]:
        name = optional_string_argument(arguments, "name_or_path")
        if not name:
            return error_text(
                "Missing name_or_path", code="missing_argument", argument="name_or_path"
            )
        kernel = _devmap_query_payload(root, "query", name_or_path=name)
        if kernel is None:
            return error_text(_NO_KERNEL_TEXT, code="graph_missing")
        # The CLI's payload shape carries `definitions` but not `matches`,
        # which this tool has always emitted and agents branch on. Switching
        # engines must not silently drop a field of the tool's contract, so
        # it is derived here from the definitions the kernel returned rather
        # than left absent (which a caller reads as zero matches).
        kernel.setdefault("matches", len(kernel.get("definitions") or []))
        return json_text(kernel)

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_graph_trace(root: Path, arguments: dict) -> list[TextContent]:
    """Path between two symbols. The kernel is the only engine.

    Measured at 1.133 s in Python against 0.202 s for the kernel.

    **The two engines differed, and the kernel is the correct one.** Python's
    BFS was *undirected* over `imports`/`calls`/`contains`/`defines`/`inherits`;
    the kernel walks resolved edges directionally. On a real probe Python
    reported a two-hop path between two functions through a shared test module
    while the kernel correctly reported no indexed path. That is why the Python
    tracer was deleted rather than kept as a fallback: a fabricated path is
    worse than an absent answer, because a caller acts on it. A capped walk says
    so rather than being reported as "no path".
    """

    def _body() -> list[TextContent]:
        start = optional_string_argument(arguments, "from")
        end = optional_string_argument(arguments, "to")
        if not start:
            return error_text("Missing from", code="missing_argument", argument="from")
        if not end:
            return error_text("Missing to", code="missing_argument", argument="to")
        kernel = _devmap_query_payload(root, "trace", start=start, end=end)
        if kernel is None:
            return error_text(_NO_KERNEL_TEXT, code="graph_missing")
        return json_text(kernel)

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_graph_impact(root: Path, arguments: dict) -> list[TextContent]:
    """Symbol-level blast radius from paths or working-tree diff.

    The last graph tool here still on `load_code_graph`. It read the whole
    graph out of the Python `index.sqlite` cache — the retired engine's read
    path, measured elsewhere in this package at 1.2 s and hundreds of MB per
    call — and then re-ran the inbound walk in Python, from a tool an agent
    reads as read-only.

    The kernel now bands its own walk (`impact --layers`), so the answer is one
    walk over one generation rather than a Python re-implementation of it, and
    it carries what Python could not: which targets matched nothing, how many
    nodes a band held beyond the ones listed, the weakest edge that reached each
    band, and whether the walk itself stopped short.
    """

    def _body() -> list[TextContent]:
        paths, list_error = optional_string_list_argument(arguments, "paths")
        if list_error:
            return list_error
        use_diff, diff_error = optional_bool_argument(arguments, "diff")
        if diff_error:
            return diff_error
        use_diff = bool(use_diff)
        if not use_diff and not paths:
            return error_text(
                "Provide paths or set diff=true",
                code="missing_argument",
                argument="paths",
            )
        seeds = [str(path) for path in (paths or [])]
        if use_diff:
            # `git diff`, not the graph — the one part of this tool that never
            # needed an engine.
            from devcouncil.indexing.graph.intel import working_tree_changed_paths

            changed = working_tree_changed_paths(root)
            if seeds:
                wanted = {seed.replace("\\", "/") for seed in seeds}
                changed = [path for path in changed if path in wanted]
            seeds = changed

        result = _devmap_query_payload(root, "impact", paths=seeds, max_depth=3)
        if result is None:
            return error_text(_NO_KERNEL_TEXT, code="graph_missing")
        if result.get("ok") is False:
            return error_text(
                f"devmap: {result.get('error') or 'refused'}", code="graph_unavailable"
            )
        result["source_paths"] = "diff" if use_diff else "paths"
        return json_text(_graph_payload(root, result))

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


#: Said when no kernel can be reached, by every graph tool here. The kernel is
#: the engine; there is no second one to fall back to.
_NO_KERNEL_TEXT = (
    "No devmap store found. Run `dev map` to build the index these tools read."
)


def _route_tool(root: Path, ask) -> list[TextContent]:
    """Run one kernel route command and wrap it in the graph-tool envelope.

    Shared by the three route tools because they differ only in which command
    they send: same unavailability rule, same envelope, same failure text. Each
    used to `load_code_graph(root)` and then run a Python re-implementation of
    the command — measured on this repository at 2.1-2.7 s against the kernel's
    0.8-1.0 s, and answering without the kernel's coverage record
    (`capabilities` on `routes`, `scan` on all three), so a client scan that
    stopped at its file cap was published as a complete inventory.
    """
    from devcouncil.devmap_client import DevMapClientError, try_connect

    client = try_connect(root)
    if client is None:
        return error_text(_NO_KERNEL_TEXT, code="graph_missing")
    try:
        result = ask(client)
    except DevMapClientError as exc:
        return error_text(f"devmap: {exc}", code="graph_unavailable")
    return json_text(_graph_payload(root, result))


async def handle_route_map(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        return _route_tool(root, lambda client: client.routes())

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_shape_check(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        route = optional_string_argument(arguments, "route")
        if route == "":
            return error_text("route must be a string", code="invalid_arguments", argument="route")
        return _route_tool(root, lambda client: client.shape_check(route_filter=route))

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)


async def handle_api_impact(root: Path, arguments: dict) -> list[TextContent]:
    def _body() -> list[TextContent]:
        route_or_path = optional_string_argument(arguments, "route_or_path")
        if not route_or_path:
            return error_text(
                "Missing route_or_path",
                code="missing_argument",
                argument="route_or_path",
            )
        return _route_tool(root, lambda client: client.api_impact(route_or_path))

    async def _run() -> list[TextContent]:
        return await asyncio.to_thread(_body)  # see "Why _body runs in a thread"

    return await with_codeintel_freshness(root, _run)
