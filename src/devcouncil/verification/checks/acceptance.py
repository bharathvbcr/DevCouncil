"""Acceptance-criteria evidence helpers extracted from Verifier."""

from __future__ import annotations

from typing import Callable, Dict, Iterable, List, Set


def coarse_proven_acceptance_ids(
    *,
    task_acceptance_ids: Iterable[str],
    successful_commands: List[str],
    command_can_prove: Callable[[str, str], bool],
) -> Set[str]:
    """Return AC ids coarse-proven by any passing acceptance-capable command."""
    if not task_acceptance_ids or not successful_commands:
        return set()
    if not any(command_can_prove("expected", cmd) for cmd in successful_commands):
        return set()
    return set(task_acceptance_ids)


def unproven_acceptance_ids(
    *,
    task_acceptance_ids: Iterable[str],
    compiled_pass: Dict[str, bool],
    coarse_proven: Set[str],
    inconclusive: Set[str],
) -> List[str]:
    """AC ids still lacking decisive proof after compiled + coarse passes."""
    unproven: List[str] = []
    for ac_id in task_acceptance_ids:
        if ac_id in inconclusive:
            continue
        if compiled_pass.get(ac_id):
            continue
        if ac_id in coarse_proven:
            continue
        unproven.append(ac_id)
    return unproven
