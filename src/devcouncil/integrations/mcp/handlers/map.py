"""Read-only MCP tools for querying ``.devcouncil/repo_map.json``."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from mcp.types import TextContent

from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.indexing.subsystem_map import (
    area_for_path,
    cross_boundary_pairs,
    dead_symbol_candidates_of,
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
        return RepoMapper(root).map_is_stale(data)
    except Exception:
        return False


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
            })
        return json_text({
            "ok": True,
            "stale": stale,
            "path": path,
            "area": resolved_area or subsystem,
            "subsystem": _subsystem_detail(sub),
        })

    subsystems = [
        _subsystem_summary(s)
        for s in (data.get("subsystems") or [])
        if isinstance(s, dict) and s.get("area")
    ]
    return json_text({
        "ok": True,
        "stale": stale,
        "languages": list(data.get("languages") or []),
        "frameworks": list(data.get("frameworks") or []),
        "package_managers": list(data.get("package_managers") or []),
        "subsystems": subsystems,
        "path": path,
        "area": resolved_area,
        "symbols": _symbols_for_path(root, path) if path else [],
    })


def _symbols_for_path(root: Path, path: str) -> list[dict[str, Any]]:
    """Per-path symbol listings from the code graph (empty when unavailable)."""
    try:
        from devcouncil.indexing.graph.build import load_code_graph

        graph = load_code_graph(root)
        if graph is None:
            return []
        norm = path.replace("\\", "/")
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
        return out[:200]
    except Exception:
        return []


async def handle_impact(root: Path, arguments: dict) -> list[TextContent]:
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
    if precise:
        try:
            from devcouncil.indexing.lsp_client import LspSessionPool

            lsp_pool = LspSessionPool(root)
        except Exception:
            lsp_pool = None

    try:
        items: list[dict[str, Any]] = []
        all_neighbor_areas: set[str] = set()
        for raw in paths:
            path = raw.replace("\\", "/")
            dependents, neighbors = impact_targets(path, data)
            resolution = "import"
            if precise and lsp_pool is not None:
                try:
                    lsp_deps = lsp_pool.dependents_of_file(path)
                    if lsp_deps is not None:
                        dependents = lsp_deps
                        resolution = "lsp"
                except Exception:
                    pass
            area = area_for_path(path, data)
            all_neighbor_areas.update(neighbors)
            item: dict[str, Any] = {
                "path": path,
                "area": area,
                "is_entry_root": is_entry_root(path, data),
                "dependents": dependents,
                "neighbors": neighbors,
            }
            if precise:
                item["resolution"] = resolution
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


async def handle_liveness(root: Path, arguments: dict) -> list[TextContent]:
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
    entry_roots = data.get("entry_roots") or []
    if not isinstance(entry_roots, list):
        entry_roots = []
    entry_roots = [str(p) for p in entry_roots]

    unwired = _filter_paths(
        unwired_candidates_of(data), data, area=area, path_prefix=path_prefix
    )
    unreachable = _filter_paths(
        unreachable_of(data), data, area=area, path_prefix=path_prefix
    )
    dead_symbols = _filter_dead_symbols(
        dead_symbol_candidates_of(data), data, area=area, path_prefix=path_prefix
    )
    if area or path_prefix:
        entry_roots = _filter_paths(entry_roots, data, area=area, path_prefix=path_prefix)

    dead_code, hidden = _structured_dead_code(
        root,
        area=area,
        path_prefix=path_prefix,
        min_confidence=min_confidence,
    )
    return json_text({
        "ok": True,
        "stale": stale,
        "area": area,
        "path_prefix": path_prefix,
        "min_confidence": min_confidence,
        "entry_roots": entry_roots,
        "unwired_candidates": unwired,
        "unreachable_files": unreachable,
        "dead_symbol_candidates": dead_symbols,
        "dead_code": dead_code,
        "dead_code_hidden": hidden,
    })


def _structured_dead_code(
    root: Path,
    *,
    area: str | None,
    path_prefix: str | None,
    min_confidence: str = "inferred",
) -> tuple[list[dict[str, Any]], int]:
    try:
        from devcouncil.indexing.graph.build import load_code_graph
        from devcouncil.indexing.graph.liveness import confidence_at_least

        graph = load_code_graph(root)
        if graph is None:
            return [], 0
        data = _load_repo_map(root) or {}
        matched: list[dict[str, Any]] = []
        hidden = 0
        for d in graph.dead_code:
            if not _matches_filters(d.path, data, area=area, path_prefix=path_prefix):
                continue
            if not confidence_at_least(d.confidence, min_confidence):
                hidden += 1
                continue
            matched.append(d.model_dump())
        return matched[:200], hidden
    except Exception:
        return [], 0


async def handle_graph_query(root: Path, arguments: dict) -> list[TextContent]:
    name = optional_string_argument(arguments, "name_or_path")
    if not name:
        return error_text(
            "Missing name_or_path", code="missing_argument", argument="name_or_path"
        )
    from devcouncil.indexing.graph import query_symbol

    return json_text({"ok": True, **query_symbol(root, name)})


async def handle_graph_trace(root: Path, arguments: dict) -> list[TextContent]:
    start = optional_string_argument(arguments, "from")
    end = optional_string_argument(arguments, "to")
    if not start:
        return error_text("Missing from", code="missing_argument", argument="from")
    if not end:
        return error_text("Missing to", code="missing_argument", argument="to")
    from devcouncil.indexing.graph import trace_path

    return json_text({"ok": True, **trace_path(root, start, end)})


async def handle_graph_impact(root: Path, arguments: dict) -> list[TextContent]:
    """Symbol-level blast radius from paths or working-tree diff (code graph)."""
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
    return json_text({"ok": True, **result})
