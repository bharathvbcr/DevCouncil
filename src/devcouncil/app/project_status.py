from __future__ import annotations

from devcouncil.artifacts.graph import ArtifactGraph


def compute_phase(graph: ArtifactGraph, persisted_phase: str | None = None) -> str:
    """Compute the current project phase, honoring explicit persisted state first."""
    if persisted_phase:
        return persisted_phase

    reqs = list(graph.requirements.values())
    tasks = list(graph.tasks.values())
    blocking_gaps = graph.blocking_gaps()
    if not reqs and not tasks:
        return "NEW"
    if reqs and not tasks:
        return "REQUIREMENTS_DRAFTED"
    if blocking_gaps:
        return "TASK_BLOCKED"
    if tasks:
        statuses = {task.status for task in tasks}
        if "running" in statuses:
            return "TASK_EXECUTING"
        if "blocked" in statuses:
            return "TASK_BLOCKED"
        if all(status in {"verified", "done"} for status in statuses):
            return "PROJECT_DONE"
        return "PLAN_APPROVED"
    return "NEW"
