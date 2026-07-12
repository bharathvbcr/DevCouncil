"""Pure helpers for reasoning about subsystem areas from a loaded ``repo_map.json``.

Shared by the prompt builder's impact block and the subsystem-boundary verification
gate so both agree on how a path maps to an area, which area is which area's neighbor,
and what a change "touches". Dependency-free and side-effect-free: everything takes an
already-parsed repo-map dict (or its ``subsystems``/``dependents`` slices) and returns
plain data. Never raises on malformed input — a missing/odd map degrades to "unknown".
"""

from __future__ import annotations

from typing import Iterable, Mapping, Sequence


def _norm(path: str) -> str:
    return str(path).replace("\\", "/")


def subsystem_areas(data: Mapping | None) -> list[str]:
    """All subsystem ``area`` prefixes declared in the map (longest first)."""
    subs = (data or {}).get("subsystems") or []
    areas = [str(s.get("area")) for s in subs if isinstance(s, dict) and s.get("area")]
    return sorted(set(areas), key=len, reverse=True)


def area_for_path(path: str, data: Mapping | None) -> str | None:
    """The subsystem area a path belongs to (longest matching area prefix), or ``None``.

    Falls back to the per-file ``area`` recorded in the map's ``files`` list when no
    subsystem prefix matches, so files outside a declared subsystem still resolve.
    """
    norm = _norm(path)
    for area in subsystem_areas(data):
        if norm == area or norm.startswith(area + "/"):
            return area
    files = (data or {}).get("files") or []
    for entry in files:
        if isinstance(entry, dict) and _norm(entry.get("path", "")) == norm:
            file_area = entry.get("area")
            return str(file_area) if file_area else None
    return None


def neighbors_for_area(area: str | None, data: Mapping | None) -> list[str]:
    """Declared neighboring areas for ``area`` (empty when unknown)."""
    if not area:
        return []
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and str(sub.get("area")) == area:
            return [str(n) for n in (sub.get("neighbors") or [])]
    return []


def are_neighbors(area_a: str | None, area_b: str | None, data: Mapping | None) -> bool:
    """True when the two areas are the same or declared neighbors (in either direction)."""
    if not area_a or not area_b:
        return True  # unknown side → don't flag
    if area_a == area_b:
        return True
    if area_b in neighbors_for_area(area_a, data):
        return True
    if area_a in neighbors_for_area(area_b, data):
        return True
    return False


def dependents_of(path: str, data: Mapping | None) -> list[str]:
    """Files that import ``path`` (reverse-import blast radius), from the map."""
    dependents = (data or {}).get("dependents") or {}
    if not isinstance(dependents, dict):
        return []
    return [str(p) for p in (dependents.get(_norm(path)) or [])]


def unwired_candidates_of(data: Mapping | None) -> list[str]:
    """Files with zero inbound edges that aren't entry roots/exempt (capped list)."""
    vals = (data or {}).get("unwired_candidates") or []
    if not isinstance(vals, list):
        return []
    return [str(p) for p in vals]


def unreachable_of(data: Mapping | None) -> list[str]:
    """Files not reachable by BFS from any entry root (capped list)."""
    vals = (data or {}).get("unreachable_files") or []
    if not isinstance(vals, list):
        return []
    return [str(p) for p in vals]


def dead_symbol_candidates_of(data: Mapping | None) -> list[str]:
    """``path:line name`` entries for unused public top-level symbols (capped)."""
    vals = (data or {}).get("dead_symbol_candidates") or []
    if not isinstance(vals, list):
        return []
    return [str(p) for p in vals]


def is_entry_root(path: str, data: Mapping | None) -> bool:
    """True when ``path`` is listed in the map's ``entry_roots``."""
    roots = (data or {}).get("entry_roots") or []
    if not isinstance(roots, list):
        return False
    norm = _norm(path)
    return any(_norm(str(r)) == norm for r in roots)


def areas_touched(paths: Iterable[str], data: Mapping | None) -> list[str]:
    """The distinct subsystem areas the given paths live in (sorted)."""
    seen: set[str] = set()
    for path in paths:
        area = area_for_path(path, data)
        if area:
            seen.add(area)
    return sorted(seen)


def impact_targets(path: str, data: Mapping | None) -> tuple[list[str], list[str]]:
    """Return ``(dependent_files, neighbor_areas)`` a change to ``path`` touches."""
    area = area_for_path(path, data)
    return dependents_of(path, data), neighbors_for_area(area, data)


def cross_boundary_pairs(
    paths: Sequence[str],
    data: Mapping | None,
) -> list[tuple[str, str]]:
    """Pairs of touched areas that are neither identical nor declared neighbors.

    Each pair ``(a, b)`` is emitted once (sorted) and signals an edit that spans two
    subsystems the map does not consider adjacent — a candidate architecture-drift
    boundary crossing.
    """
    areas = areas_touched(paths, data)
    crossings: set[tuple[str, str]] = set()
    for i, area_a in enumerate(areas):
        for area_b in areas[i + 1:]:
            if not are_neighbors(area_a, area_b, data):
                crossings.add(tuple(sorted((area_a, area_b))))  # type: ignore[arg-type]
    return sorted(crossings)
