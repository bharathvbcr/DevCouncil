"""Shared verdict classification for JSON and evidence exports."""

from __future__ import annotations

from typing import Any, Literal

from devcouncil.artifacts.graph import ArtifactGraph

Verdict = Literal["passed", "blocked", "incomplete"]
IncompleteKind = Literal["quality", "infra"]

_INFRA_GAP_TYPES = frozenset({"invalid_verification_command"})
_INFRA_DESC_MARKERS = (
    "sdk is not installed",
    "not found on path",
    "session limit",
    "rate limit",
    "too many requests",
    "limit_rpm",
    "unknown agent profile",
    "failed to start or execute",
)


def _gaps_from_graph(graph: ArtifactGraph) -> list[Any]:
    blocking = list(graph.blocking_gaps())
    all_gaps = getattr(graph, "all_gaps", None)
    if callable(all_gaps):
        return list(all_gaps())
    return blocking


def classify_incomplete_kind(
    graph: ArtifactGraph,
    *,
    agent_run: dict[str, Any] | None = None,
) -> IncompleteKind:
    """Separate infra incomplete (executor/SDK/session) from quality incomplete."""
    if agent_run:
        returncode = agent_run.get("returncode")
        if returncode not in (None, 0):
            parts: list[str] = [str(agent_run.get("status") or "")]
            for key in ("stderr_preview", "stdout_preview"):
                val = agent_run.get(key)
                if isinstance(val, list):
                    parts.extend(str(x) for x in val)
                elif val:
                    parts.append(str(val))
            hay = " ".join(parts).lower()
            if any(marker in hay for marker in _INFRA_DESC_MARKERS):
                return "infra"

    for gap in _gaps_from_graph(graph):
        gap_type = getattr(gap, "gap_type", "") or ""
        if gap_type in _INFRA_GAP_TYPES:
            return "infra"
        desc = (getattr(gap, "description", "") or "").lower()
        if any(marker in desc for marker in _INFRA_DESC_MARKERS):
            return "infra"
    return "quality"


def classify_verdict(
    graph: ArtifactGraph,
    *,
    live_blockers: int = 0,
    agent_run: dict[str, Any] | None = None,
) -> tuple[Verdict, IncompleteKind | None]:
    """Return ``(verdict, incomplete_kind)`` where kind is set only for incomplete."""
    summary = graph.coverage_summary()
    if summary["blocking_gaps"] > 0 or live_blockers > 0:
        return "blocked", None
    if summary["ac_without_evidence"] > 0:
        return "incomplete", classify_incomplete_kind(graph, agent_run=agent_run)
    return "passed", None
