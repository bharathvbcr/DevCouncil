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


def neighbors_established(data: Mapping | None) -> bool:
    """Whether this map carries evidence that subsystem neighbors were computed.

    Two independent positive claims, either of which counts: the producer's own
    provenance marker at ``meta.devmap_rust.neighbors_computed``, and a
    non-empty ``neighbors`` list anywhere in ``subsystems`` — a map that names
    one adjacency demonstrably computed them. The second is what answers for a
    map written before the marker existed.

    Absence of both is absence of evidence, not evidence of absence. The kernel
    emitted ``"neighbors": []`` as a literal for every subsystem for the whole
    life of the field, and it has been the only map writer since the Python one
    was retired, so on any map built before that was fixed an empty field says
    "this producer does not compute the field" rather than "this repository has
    no adjacent subsystems". Reading the first as the second is how
    :func:`are_neighbors` came to answer *not adjacent* for a relation nothing
    ever measured.
    """
    meta = (data or {}).get("meta")
    if isinstance(meta, Mapping):
        marker = meta.get("devmap_rust")
        if isinstance(marker, Mapping) and marker.get("neighbors_computed") is True:
            return True
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and sub.get("neighbors"):
            return True
    return False


def _neighbors_answer_is_partial(data: Mapping | None) -> bool:
    """Whether the map says its neighbor lists are less than the whole relation.

    Two positive claims, read the same way :func:`_entry_roots_truncated` reads
    its own: the per-area lists were capped (the writer bounds them because a
    directory-level area can couple to hundreds), or some coupling edge named an
    endpoint the generation could not place in any area. Either way a *listed*
    neighbor is still a neighbor, but an absent one is no longer evidence of
    absence — so the negative degrades to unknown while the positive stands.

    Only a positive claim counts. A map carrying no such metadata claims no
    truncation and is answered as complete; the producer is the only thing that
    can know.
    """
    meta = (data or {}).get("liveness_meta")
    if not isinstance(meta, Mapping):
        return False
    subs = meta.get("subsystems")
    if not isinstance(subs, Mapping):
        return False
    if subs.get("neighbors_truncated") is True:
        return True
    unresolved = subs.get("neighbors_endpoints_unresolved")
    if isinstance(unresolved, int) and not isinstance(unresolved, bool) and unresolved > 0:
        return True
    shown, total = subs.get("neighbors_shown"), subs.get("neighbors_total")
    if isinstance(shown, int) and isinstance(total, int) and not isinstance(shown, bool):
        return total > shown
    return False


def can_rule_out_adjacency(data: Mapping | None) -> bool:
    """Whether an area's absence from another's neighbor list is evidence.

    The question every consumer of a *negative* adjacency answer is really
    asking, and it takes both halves: :func:`neighbors_established` says the
    producer computed the relation, and it must also claim to hold all of it.
    Checking only the first pushes the same defect one level up — a map that
    computed neighbors but capped the lists would let a caller read "no
    crossings found" off a relation it only partly has.
    """
    return neighbors_established(data) and not _neighbors_answer_is_partial(data)


def are_neighbors(
    area_a: str | None,
    area_b: str | None,
    data: Mapping | None,
) -> bool | None:
    """Whether two areas are adjacent: ``True``/``False``, or ``None`` for unknown.

    ``None`` means the map never established the neighbor relation, so the
    honest answer is neither yes nor no — the same rule and the same tri-state
    :func:`is_entry_root` uses for a capped list, for the same reason: a
    negative is only evidence when the producer was in a position to give one.

    ``None`` also covers a relation the map only partly holds: the writer caps
    each area's list and reports what it could not place, and a negative read
    off a capped sample is a capped sample answering as a complete one.

    The positive answers need no such evidence and stay definite. Two names for
    one area are adjacent by identity, an unknown side is not something to flag,
    and an area listed as a neighbor is listed whatever else the map omits.
    """
    if not area_a or not area_b:
        return True  # unknown side → don't flag
    if area_a == area_b:
        return True
    if area_b in neighbors_for_area(area_a, data):
        return True
    if area_a in neighbors_for_area(area_b, data):
        return True
    return False if can_rule_out_adjacency(data) else None


def handoff_paths_established(data: Mapping | None) -> bool:
    """Whether this map carries evidence that subsystem handoff paths were computed.

    The same two independent positive claims :func:`neighbors_established`
    accepts, for the same field-shaped defect: the producer's own provenance
    marker at ``meta.devmap_rust.handoff_paths_computed``, and a non-empty
    ``handoff_paths`` list anywhere in ``subsystems`` — a map that names one
    crossing demonstrably computed them. The second is what answers for a map
    written before the marker existed.

    The kernel emitted ``"handoff_paths": []`` as a literal for every subsystem
    for the whole life of the field, and it has been the only map writer since
    the Python one was retired, so on any map built before that was fixed an
    empty field says "this producer does not compute the field" rather than
    "nothing here reaches another subsystem". That distinction matters more for
    this field than for most: ``indexing/map_artifacts.py`` writes step 6 of the
    generated agent guide as "Use ``neighbors`` and ``handoff_paths`` in
    ``subsystems`` to follow cross-subsystem flow", so the guide sent agents to
    a stub and the stub answered "no flow".
    """
    meta = (data or {}).get("meta")
    if isinstance(meta, Mapping):
        marker = meta.get("devmap_rust")
        if isinstance(marker, Mapping) and marker.get("handoff_paths_computed") is True:
            return True
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and sub.get("handoff_paths"):
            return True
    return False


def _handoff_paths_answer_is_partial(data: Mapping | None) -> bool:
    """Whether the map says its handoff lists are less than the whole relation.

    Read exactly as :func:`_neighbors_answer_is_partial` reads its own, off the
    field's own counters — and it must be its own question rather than a reuse
    of the neighbour one, because the two cap different populations: an area
    with two neighbors can reach them through fifty file pairs, so
    ``neighbors_truncated: false`` says nothing about whether the handoff lists
    are whole. The unplaceable-endpoint count is shared, because one sweep
    produces both relations and rejects the same edges from each.

    Only a positive claim counts. A map carrying no such metadata claims no
    truncation and is answered as complete; the producer is the only thing that
    can know.
    """
    meta = (data or {}).get("liveness_meta")
    if not isinstance(meta, Mapping):
        return False
    subs = meta.get("subsystems")
    if not isinstance(subs, Mapping):
        return False
    if subs.get("handoff_paths_truncated") is True:
        return True
    unresolved = subs.get("neighbors_endpoints_unresolved")
    if isinstance(unresolved, int) and not isinstance(unresolved, bool) and unresolved > 0:
        return True
    shown, total = subs.get("handoff_paths_shown"), subs.get("handoff_paths_total")
    if isinstance(shown, int) and isinstance(total, int) and not isinstance(shown, bool):
        return total > shown
    return False


def can_rule_out_handoffs(data: Mapping | None) -> bool:
    """Whether an empty ``handoff_paths`` is evidence that nothing crosses.

    Both halves, the way :func:`can_rule_out_adjacency` takes both:
    :func:`handoff_paths_established` says the producer computed the relation,
    and it must also claim to hold all of it. A map that computed handoffs but
    capped the lists cannot support "this subsystem reaches nothing else".
    """
    return handoff_paths_established(data) and not _handoff_paths_answer_is_partial(data)


def handoff_paths_for_area(area: str | None, data: Mapping | None) -> list[str] | None:
    """Declared handoff paths for ``area``: a list, or ``None`` for unknown.

    ``None`` is the honest answer when the map never established the relation,
    or holds only part of it — the same tri-state, and the same reason, as
    :func:`are_neighbors` and :func:`is_entry_root`. A *non-empty* list is
    definite whatever else the map omits; it is only the empty one that has to
    prove the producer was in a position to give it.
    """
    if not area:
        return None
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and str(sub.get("area")) == area:
            listed = [str(h) for h in (sub.get("handoff_paths") or [])]
            if listed:
                return listed
            return [] if can_rule_out_handoffs(data) else None
    return None


def role_files_established(data: Mapping | None) -> bool:
    """Whether this map carries evidence that subsystem role buckets were computed.

    The same two positive claims :func:`neighbors_established` and
    :func:`handoff_paths_established` accept, for the third field that carried
    the identical defect: the producer's marker at
    ``meta.devmap_rust.role_files_computed``, and a non-empty ``role_files``
    anywhere in ``subsystems``.

    The kernel emitted ``"role_files": {}`` for every subsystem for the field's
    whole life, so on any map built before that was fixed an empty mapping says
    "this producer does not compute the field" rather than "this subsystem has
    no recognisable roles". Four readers took the generated agent guide's step 5
    at its word and got nothing back, one of them functionally:
    :mod:`devcouncil.verification.test_resolver` resolves a subsystem's tests
    through ``role_files["tests"]``, so a change with no direct test importer
    had no fallback at all.
    """
    meta = (data or {}).get("meta")
    if isinstance(meta, Mapping):
        marker = meta.get("devmap_rust")
        if isinstance(marker, Mapping) and marker.get("role_files_computed") is True:
            return True
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and sub.get("role_files"):
            return True
    return False


def _role_files_answer_is_partial(data: Mapping | None) -> bool:
    """Whether the map says its role buckets are less than the whole inventory.

    Almost always ``True`` where the field is computed at all, and that is the
    point: the buckets are capped per role, so they are a sample for
    orientation. Read off the field's own counters, never the neighbour or
    handoff ones — the three cap different populations.
    """
    meta = (data or {}).get("liveness_meta")
    if not isinstance(meta, Mapping):
        return False
    subs = meta.get("subsystems")
    if not isinstance(subs, Mapping):
        return False
    if subs.get("role_files_truncated") is True:
        return True
    shown, total = subs.get("role_files_shown"), subs.get("role_files_total")
    if isinstance(shown, int) and isinstance(total, int) and not isinstance(shown, bool):
        return total > shown
    return False


def role_files_are_complete(data: Mapping | None) -> bool:
    """Whether a role bucket may be read as the subsystem's whole inventory.

    Both halves, as :func:`can_rule_out_adjacency` takes both. In practice this
    is ``False`` on any real map, which is the honest answer: ``role_files``
    is documented as a capped sample, and a consumer that needs completeness
    goes to ``files`` or reads ``role_file_counts`` for the real total.
    """
    return role_files_established(data) and not _role_files_answer_is_partial(data)


def role_files_for_area(area: str | None, data: Mapping | None) -> dict[str, list[str]] | None:
    """Declared role buckets for ``area``: a mapping, or ``None`` for unknown.

    ``None`` means the map never established the relation, so neither "these are
    the roles" nor "there are none" is supportable. A *populated* mapping is
    returned as-is and is definite — it is only the empty one that has to prove
    the producer was in a position to give it.
    """
    if not area:
        return None
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and str(sub.get("area")) == area:
            listed = sub.get("role_files")
            if isinstance(listed, Mapping) and listed:
                return {str(k): [str(p) for p in (v or [])] for k, v in listed.items()}
            return {} if role_files_established(data) else None
    return None


def role_file_total(area: str | None, role: str, data: Mapping | None) -> int | None:
    """The real number of files in ``area``'s ``role`` bucket, or ``None``.

    ``role_files[role]`` is capped at a handful of examples, so its length is
    the size of a sample and never an inventory. This reads the companion
    ``role_file_counts`` the producer writes beside it; ``None`` means the map
    does not carry the total, and the sample must not be reported as one.
    """
    if not area:
        return None
    for sub in (data or {}).get("subsystems") or []:
        if isinstance(sub, dict) and str(sub.get("area")) == area:
            counts = sub.get("role_file_counts")
            if isinstance(counts, Mapping):
                value = counts.get(role)
                if isinstance(value, int) and not isinstance(value, bool):
                    return value
            return None
    return None


def dependents_of(path: str, data: Mapping | None) -> list[str]:
    """Files that import ``path`` (reverse-import blast radius), from the map.

    The list may be truncated by whichever writer produced the map — the Rust kernel
    since the Python engine's retirement; see :func:`dependents_total_of` for the full
    importer count when truncated.
    """
    dependents = (data or {}).get("dependents") or {}
    if not isinstance(dependents, dict):
        return []
    return [str(p) for p in (dependents.get(_norm(path)) or [])]


def dependents_total_of(path: str, data: Mapping | None) -> int | None:
    """Full importer count when ``dependents`` was truncated; else ``None``.

    When this returns an int greater than ``len(dependents_of(...))``, the listed
    dependents are a sample — not the complete blast radius.
    """
    totals = (data or {}).get("dependents_total") or {}
    if not isinstance(totals, dict):
        return None
    raw = totals.get(_norm(path))
    if raw is None:
        return None
    try:
        n = int(raw)
    except (TypeError, ValueError):
        return None
    return n if n > 0 else None


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


def is_entry_root(path: str, data: Mapping | None) -> bool | None:
    """Whether ``path`` is an entry root: ``True``/``False``, or ``None`` for unknown.

    The kernel caps ``entry_roots`` at a token budget before writing
    ``repo_map.json``, so absence from the list is only evidence of *not* being
    an entry root when the list is known to be complete. A genuine entry root
    sorting past the cap used to be reported as a flat ``False`` — a capped
    sample answering as a complete one, which is how "not checked" comes to read
    as "checked and negative".

    Presence stays provable either way: a path *in* the list is an entry root
    whether or not the list was cut short.

    ``None`` means unknown, and the caller emits it as a JSON ``null`` rather
    than picking a side. A map whose ``liveness_meta.entry_roots`` says
    ``truncated`` (or whose ``shown`` is below its ``total``) cannot answer a
    negative; a map that says it is complete can. A map carrying no such
    metadata claims no truncation and is answered as before — the producer is
    the only thing that can know, and it now says so.
    """
    roots = (data or {}).get("entry_roots") or []
    if not isinstance(roots, list):
        return False
    norm = _norm(path)
    if any(_norm(str(r)) == norm for r in roots):
        return True
    return None if _entry_roots_truncated(data) else False


def _entry_roots_truncated(data: Mapping | None) -> bool:
    """Whether the map states that its ``entry_roots`` list was cut short.

    Only a positive claim counts. Both the explicit ``truncated`` flag and a
    ``shown``/``total`` disagreement are read, because a producer may carry one
    without the other; anything missing or malformed is not a claim, and never
    invents one.
    """
    meta = (data or {}).get("liveness_meta")
    if not isinstance(meta, Mapping):
        return False
    entry = meta.get("entry_roots")
    if not isinstance(entry, Mapping):
        return False
    if entry.get("truncated") is True:
        return True
    shown, total = entry.get("shown"), entry.get("total")
    if isinstance(shown, int) and isinstance(total, int) and not isinstance(shown, bool):
        return total > shown
    return False


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
            # `is False`, not falsiness: an unknown relation (``None``) is not a
            # crossing. Callers that need to tell "no crossings" from "could not
            # look" ask :func:`can_rule_out_adjacency` — see the boundary gate.
            if are_neighbors(area_a, area_b, data) is False:
                crossings.add(tuple(sorted((area_a, area_b))))  # type: ignore[arg-type]
    return sorted(crossings)
