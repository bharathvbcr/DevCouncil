"""Advisory subsystem-boundary gate — architecture drift over the mapped area graph.

The repo map already knows which subsystems are *neighbors* (they hand off to each
other). A change that edits files in two subsystems the map does NOT consider neighbors,
and that the task plan never declared it would span, is a candidate architecture-drift
crossing: business logic leaking into a UI layer, a storage change reaching into the
council prompts, etc.

This gate flags those crossings. It is **advisory (non-blocking) by default** — an
undeclared crossing is often legitimate, so it should inform review rather than halt the
loop — but is ``blocking``-configurable for teams that want a hard architectural
boundary. It reads only ``repo_map.json`` (no model calls, no subprocess) and degrades to
a no-op when the map lacks ``subsystems``/``neighbors``.

"Plan coverage": a crossing between areas A and B is covered when the task's *planned
files* already live in both A and B — i.e. the plan declared the change would touch both
subsystems. Only crossings that reach an area the plan did not declare are flagged.
"""

from __future__ import annotations

import logging
from typing import Callable, List, Mapping, Sequence

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.indexing.subsystem_map import (
    area_for_path,
    areas_touched,
    cross_boundary_pairs,
)

logger = logging.getLogger(__name__)

# Cap emitted crossings so a sprawling change can't flood the gap list.
_MAX_CROSSINGS = 6


def detect_subsystem_boundary_gaps(
    *,
    task: Task,
    changed_files: Sequence[str],
    repo_map: Mapping | None,
    next_gap_id: Callable[[str, str], str],
    blocking: bool = False,
) -> List[Gap]:
    """Flag edits that cross non-neighbor subsystem boundaries without plan coverage.

    Returns advisory (or blocking, per ``blocking``) architecture-drift gaps — one per
    undeclared crossing pair. Empty when the map is missing/degenerate, the change stays
    within one area, or every crossing was already declared by the plan.
    """
    if not repo_map or not (repo_map.get("subsystems") if hasattr(repo_map, "get") else None):
        return []

    changed = [p.replace("\\", "/") for p in changed_files if p and p.strip()]
    if len(areas_touched(changed, repo_map)) < 2:
        return []

    planned_paths = [pf.path.replace("\\", "/") for pf in task.planned_files]
    planned_areas = set(areas_touched(planned_paths, repo_map))

    crossings = cross_boundary_pairs(changed, repo_map)
    if not crossings:
        return []

    gaps: List[Gap] = []
    for area_a, area_b in crossings[:_MAX_CROSSINGS]:
        # Covered when the plan declared files in BOTH areas of the crossing.
        if area_a in planned_areas and area_b in planned_areas:
            continue
        undeclared = sorted({area_a, area_b} - planned_areas)
        files_a = [f for f in changed if area_for_path(f, repo_map) == area_a]
        files_b = [f for f in changed if area_for_path(f, repo_map) == area_b]
        evidence = [f"{area_a}: " + ", ".join(files_a[:5])] if files_a else []
        if files_b:
            evidence.append(f"{area_b}: " + ", ".join(files_b[:5]))
        gaps.append(Gap(
            id=next_gap_id(task.id, "BOUNDARY"),
            severity="medium",
            gap_type="architecture_drift",
            task_id=task.id,
            description=(
                f"Change spans non-neighbor subsystems `{area_a}` and `{area_b}` "
                f"but the plan did not declare it would touch {', '.join(f'`{a}`' for a in undeclared)}. "
                "Cross-boundary edits between areas the repo map does not consider "
                "adjacent are a common source of architecture drift."
            ),
            evidence=evidence or [f"{area_a} ⇄ {area_b}"],
            recommended_fix=(
                "If this crossing is intended, declare planned files in both subsystems "
                "(or split the work into per-subsystem tasks); otherwise keep the edit "
                "within its subsystem and route through the neighboring interface. "
                "Run `dev map` if the subsystem neighbors are out of date."
            ),
            blocking=blocking,
        ))
    if gaps:
        logger.info(
            "subsystem-boundary gate: %d undeclared crossing(s) for task %s (blocking=%s)",
            len(gaps), task.id, blocking,
        )
    return gaps
