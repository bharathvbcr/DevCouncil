"""Read-only MCP tools for querying ``.devcouncil/repo_map.json``."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any, NamedTuple

from mcp.types import TextContent

from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.indexing.subsystem_map import (
    area_for_path,
    cross_boundary_pairs,
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

    ``query_symbol``/``trace_path``/``route_map``/``api_impact`` answer with
    ``{"error": "no code graph; run `dev map` first"}``. Prepending a literal
    ``"ok": True`` published that failure as a success, so a caller branching on
    ``ok`` read "the graph has no callers for this symbol" from a response that
    means "there is no graph".
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


def _subsystem_detail(sub: dict[str, Any]) -> dict[str, Any]:
    role_files = sub.get("role_files") or {}
    if not isinstance(role_files, dict):
        role_files = {}
    return {
        "area": sub.get("area"),
        "summary": sub.get("summary") or "",
        "entry_points": list(sub.get("entry_points") or []),
        "critical_files": list(sub.get("critical_files") or []),
        "neighbors": list(sub.get("neighbors") or []),
        "handoff_paths": list(sub.get("handoff_paths") or []),
        "role_files": {str(k): list(v or []) for k, v in role_files.items()},
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
    async def _run() -> list[TextContent]:
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
                "subsystem": _subsystem_detail(sub),
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
            "symbols_truncated": False,
            "symbols_reason": "not requested: no path argument",
        }
    symbols = _symbols_for_path(root, path)
    fields: dict[str, Any] = {
        "symbols": symbols.items,
        "symbols_available": symbols.ok,
        "symbols_source": symbols.source,
        "symbols_shown": len(symbols.items),
        "symbols_total": symbols.total if symbols.ok else None,
        "symbols_truncated": symbols.truncated,
    }
    if not symbols.ok:
        fields["symbols_reason"] = symbols.reason
    return fields


class SymbolScan(NamedTuple):
    """A per-path symbol listing plus whether the listing could be produced.

    ``ok=False`` means no engine answered — never "this file has no symbols".
    The two used to be the same empty list.
    """

    ok: bool
    source: str
    reason: str
    items: list[dict[str, Any]]
    total: int
    truncated: bool


def _scan_ok(items: list[dict[str, Any]], source: str) -> SymbolScan:
    shown = items[:_ROW_LIMIT]
    return SymbolScan(
        ok=True,
        source=source,
        reason="",
        items=shown,
        total=len(items),
        truncated=len(shown) < len(items),
    )


def _symbols_for_path(root: Path, path: str) -> SymbolScan:
    """Per-path symbol listings from the code graph, or an explicit failure.

    Both engines are tried in order. Every reason one of them declined is kept
    and reported: a missing kernel, a locked store, a mid-build truncation and a
    file that genuinely defines nothing all used to return the same ``[]``.
    """
    norm = path.replace("\\", "/")
    reasons: list[str] = []
    try:
        from devcouncil.devmap_client import (
            DevMapClientError,
            resolution_unavailable_reason,
            try_connect,
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
            return _scan_ok(out, "devmap")
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        reasons.append(f"devmap: {exc}")
    try:
        from devcouncil.indexing.graph.build import load_code_graph

        graph = load_code_graph(root)
        if graph is None:
            reasons.append("no code graph (run `dev map`)")
            return SymbolScan(False, "", "; ".join(reasons), [], 0, False)
        out = []
        for n in graph.nodes:
            if n.path != norm:
                continue
            kind = n.kind.value if hasattr(n.kind, "value") else str(n.kind)
            if kind == "file":
                continue
            out.append(
                {
                    "id": n.id,
                    "kind": kind,
                    "name": n.name,
                    "line": n.line,
                }
            )
        return _scan_ok(out, "code_graph")
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        reasons.append(f"code graph: {exc}")
        return SymbolScan(False, "", "; ".join(reasons), [], 0, False)


async def handle_impact(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
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
        lsp_pool = None
        lsp_pool_error = ""
        if precise:
            try:
                from devcouncil.indexing.lsp_client import LspSessionPool

                lsp_pool = LspSessionPool(root)
            except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                lsp_pool = None
                lsp_pool_error = str(exc)

        try:
            items: list[dict[str, Any]] = []
            all_neighbor_areas: set[str] = set()
            for raw in paths:
                path = raw.replace("\\", "/")
                dependents, neighbors = impact_targets(path, data)
                # `import` is the repo-map heuristic — the weakest of the three.
                # Which engine actually answered rides on every item now: it used
                # to be emitted only under `precise`, so a kernel failure left
                # heuristic dependents looking like a symbol-level answer.
                resolution = "import"
                resolution_reason = ""
                fallback = "; dependents come from the repo-map import heuristic"
                if not precise:
                    try:
                        from devcouncil.devmap_client import (
                            DevMapClientError,
                            resolution_unavailable_reason,
                            try_connect,
                        )

                        client = try_connect(root)
                        if client is None:
                            resolution_reason = (
                                "no built devmap store (run `dev map`)" + fallback
                            )
                        else:
                            resp = client.impact(path, depth=1)
                            reason = resolution_unavailable_reason(resp.resolution)
                            if reason:
                                raise DevMapClientError(reason)
                            rust_deps = sorted({
                                str(edge.get("source_file") or "")
                                for edge in resp.items
                                if edge.get("source_file")
                            })
                            if rust_deps:
                                dependents = rust_deps
                                resolution = "devmap"
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
                for a, b in cross_boundary_pairs(paths, data)
            ]
            payload: dict[str, Any] = {
                "ok": True,
                "stale": stale,
                "paths": items,
                "neighbor_areas": sorted(all_neighbor_areas),
                "cross_boundary_pairs": crossings,
            }
            if precise:
                payload["precise"] = True
            return json_text(payload)
        finally:
            if lsp_pool is not None:
                lsp_pool.close()

    return await with_codeintel_freshness(root, _run)


async def handle_liveness(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
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
            "dead_code_truncated": dead_scan.truncated,
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
        return json_text(payload)

    return await with_codeintel_freshness(root, _run)


class DeadCodeScan(NamedTuple):
    """A dead-code listing plus proof that the scan actually ran.

    ``ok=False`` means no engine produced a listing — a missing kernel, a
    locked store, a mid-build truncation or a raising graph load. It never
    means "this repository has no dead code", which is what the old
    ``([], 0)`` said in all four cases.

    ``total`` counts every candidate the scan saw within the requested scope,
    before the confidence filter and before the row cap, so a caller can always
    reconstruct what it is not being shown.
    """

    ok: bool
    source: str
    reason: str
    items: list[dict[str, Any]]
    total: int
    hidden_low_confidence: int
    truncated: bool

    @property
    def hidden(self) -> int:
        """Candidates in scope that this response does not carry."""
        return self.total - len(self.items)


def _dead_scan_ok(
    matched: list[dict[str, Any]], *, source: str, total: int, hidden_low_confidence: int
) -> DeadCodeScan:
    shown = matched[:_ROW_LIMIT]
    return DeadCodeScan(
        ok=True,
        source=source,
        reason="",
        items=shown,
        total=total,
        hidden_low_confidence=hidden_low_confidence,
        truncated=len(shown) < len(matched),
    )


def _structured_dead_code(
    root: Path,
    *,
    area: str | None,
    path_prefix: str | None,
    min_confidence: str = "inferred",
) -> DeadCodeScan:
    data = _load_repo_map(root) or {}
    reasons: list[str] = []
    try:
        from devcouncil.devmap_client import (
            DevMapClientError,
            resolution_unavailable_reason,
            try_connect,
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
            return _dead_scan_ok(
                matched, source="devmap", total=in_scope, hidden_low_confidence=hidden
            )
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        reasons.append(f"devmap: {exc}")
    try:
        from devcouncil.indexing.graph.build import load_code_graph
        from devcouncil.indexing.graph.liveness import confidence_at_least

        graph = load_code_graph(root)
        if graph is None:
            reasons.append("no code graph (run `dev map`)")
            return DeadCodeScan(False, "", "; ".join(reasons), [], 0, 0, False)
        matched = []
        in_scope = 0
        hidden = 0
        for d in graph.dead_code:
            if not _matches_filters(d.path, data, area=area, path_prefix=path_prefix):
                continue
            in_scope += 1
            if not confidence_at_least(d.confidence, min_confidence):
                hidden += 1
                continue
            matched.append(d.model_dump())
        return _dead_scan_ok(
            matched, source="code_graph", total=in_scope, hidden_low_confidence=hidden
        )
    except Exception as exc:  # noqa: BLE001 - reported, not swallowed
        reasons.append(f"code graph: {exc}")
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
    async def _run() -> list[TextContent]:
        query = optional_string_argument(arguments, "query")
        if not query:
            return error_text("Missing query", code="missing_argument", argument="query")
        from devcouncil.indexing.graph.cypher import run_cypher

        return json_text(run_cypher(root, query))

    return await with_codeintel_freshness(root, _run)


async def handle_pdg_query(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
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

    return await with_codeintel_freshness(root, _run)


async def handle_explain(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
        path = optional_string_argument(arguments, "path")
        category = optional_string_argument(arguments, "category")
        from devcouncil.indexing.graph.query import explain_pdg_taint

        return json_text(explain_pdg_taint(root, path=path or None, category=category or None))

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
    """Symbol lookup, kernel-first.

    This answered entirely from Python: `query_symbol` -> `load_code_graph` ->
    `index.sqlite`, re-materialising 14,057 nodes and 71,195 edges as pydantic
    models on **every** call. Measured at 1.057 s against 0.154 s for the
    kernel-backed `devcouncil_code_search` answering the same class of question,
    with ~90% of it in that re-materialisation — and the first call after any
    kernel build additionally cost 6.5-7.5 s and wrote 242 MB of SQLite under a
    writer lease, from a tool an agent reads as read-only.
    """

    async def _run() -> list[TextContent]:
        name = optional_string_argument(arguments, "name_or_path")
        if not name:
            return error_text(
                "Missing name_or_path", code="missing_argument", argument="name_or_path"
            )
        kernel = await asyncio.to_thread(
            _devmap_query_payload, root, "query", name_or_path=name
        )
        if kernel is not None:
            # The CLI's payload shape carries `definitions` but not `matches`,
            # which this tool has always emitted and agents branch on. Switching
            # engines must not silently drop a field of the tool's contract, so
            # it is derived here from the definitions the kernel returned rather
            # than left absent (which a caller reads as zero matches).
            kernel.setdefault("matches", len(kernel.get("definitions") or []))
            return json_text(kernel)
        from devcouncil.indexing.graph import query_symbol

        payload = _graph_payload(root, query_symbol(root, name))
        # Provenance is not optional here. Two engines can answer this tool and
        # they do not agree in every case, so a caller that cannot tell which
        # replied cannot interpret the answer.
        payload.setdefault("source", "code_graph")
        return json_text(payload)

    return await with_codeintel_freshness(root, _run)


async def handle_graph_trace(root: Path, arguments: dict) -> list[TextContent]:
    """Path between two symbols, kernel-first.

    Measured at 1.133 s in Python against 0.202 s for the kernel.

    **The two engines differ, and the kernel is the correct one.** Python's BFS
    is *undirected* over `imports`/`calls`/`contains`/`defines`/`inherits`; the
    kernel walks resolved edges directionally. On a real probe Python reported a
    two-hop path between two functions through a shared test module while the
    kernel correctly reported no indexed path. Agents will see fewer, truer
    paths — and, since the kernel pass in this session, a capped walk now says
    so rather than being reported as "no path".
    """

    async def _run() -> list[TextContent]:
        start = optional_string_argument(arguments, "from")
        end = optional_string_argument(arguments, "to")
        if not start:
            return error_text("Missing from", code="missing_argument", argument="from")
        if not end:
            return error_text("Missing to", code="missing_argument", argument="to")
        kernel = await asyncio.to_thread(
            _devmap_query_payload, root, "trace", start=start, end=end
        )
        if kernel is not None:
            return json_text(kernel)
        from devcouncil.indexing.graph import trace_path

        payload = _graph_payload(root, trace_path(root, start, end))
        payload.setdefault("source", "code_graph")
        return json_text(payload)

    return await with_codeintel_freshness(root, _run)


async def handle_graph_impact(root: Path, arguments: dict) -> list[TextContent]:
    """Symbol-level blast radius from paths or working-tree diff (code graph)."""

    async def _run() -> list[TextContent]:
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

        from devcouncil.indexing.graph.build import load_code_graph
        from devcouncil.indexing.graph.intel import diff_impact

        graph = load_code_graph(root)
        if graph is None:
            return error_text(
                "No code graph found. Run `dev map` to generate .devcouncil/graph/code_graph.json.",
                code="graph_missing",
            )
        result = diff_impact(
            root,
            graph,
            paths=paths,
            use_diff=use_diff,
            max_depth=3,
        )
        return json_text(_graph_payload(root, result))

    return await with_codeintel_freshness(root, _run)


async def handle_route_map(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
        from devcouncil.indexing.graph.api_routes import route_map
        from devcouncil.indexing.graph.build import load_code_graph

        graph = load_code_graph(root)
        if graph is None:
            return error_text(
                "No code graph found. Run `dev map` to generate .devcouncil/graph/code_graph.json.",
                code="graph_missing",
            )
        return json_text(_graph_payload(root, route_map(root, graph)))

    return await with_codeintel_freshness(root, _run)


async def handle_shape_check(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
        route = optional_string_argument(arguments, "route")
        if route == "":
            return error_text("route must be a string", code="invalid_arguments", argument="route")
        from devcouncil.indexing.graph.api_routes import shape_check
        from devcouncil.indexing.graph.build import load_code_graph

        graph = load_code_graph(root)
        if graph is None:
            return error_text(
                "No code graph found. Run `dev map` to generate .devcouncil/graph/code_graph.json.",
                code="graph_missing",
            )
        return json_text(_graph_payload(root, shape_check(root, graph, route_filter=route)))

    return await with_codeintel_freshness(root, _run)


async def handle_api_impact(root: Path, arguments: dict) -> list[TextContent]:
    async def _run() -> list[TextContent]:
        route_or_path = optional_string_argument(arguments, "route_or_path")
        if not route_or_path:
            return error_text(
                "Missing route_or_path",
                code="missing_argument",
                argument="route_or_path",
            )
        from devcouncil.indexing.graph.api_routes import api_impact
        from devcouncil.indexing.graph.build import load_code_graph

        graph = load_code_graph(root)
        if graph is None:
            return error_text(
                "No code graph found. Run `dev map` to generate .devcouncil/graph/code_graph.json.",
                code="graph_missing",
            )
        return json_text(_graph_payload(root, api_impact(root, route_or_path, graph)))

    return await with_codeintel_freshness(root, _run)
