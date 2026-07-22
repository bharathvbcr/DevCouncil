"""Community labeling helpers (leaf module — no intel/verifier imports)."""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind

COMMUNITY_TIMEOUT_SECONDS = 15.0


def store_health_from_state(state: str) -> str:
    """Map codeintel store state to canonical-store health for limit reports."""
    if state == "committed":
        return "healthy"
    if state in {"corrupt", "empty", "uninitialized"}:
        return state
    return state or "unknown"


@dataclass(frozen=True)
class GraphLimitDegraded:
    kind: str
    reason: str
    canonical_store_health: str
    recovery_command: str
    detail: Optional[str] = None

    def as_dict(self) -> Dict[str, Any]:
        payload: Dict[str, Any] = {
            "degraded": True,
            "kind": self.kind,
            "reason": self.reason,
            "canonical_store_health": self.canonical_store_health,
            "recovery_command": self.recovery_command,
        }
        if self.detail:
            payload["detail"] = self.detail
        return payload


def compatibility_export_limit(
    *,
    canonical_store_health: str,
    reason: str,
    detail: Optional[str] = None,
) -> GraphLimitDegraded:
    return GraphLimitDegraded(
        kind="compatibility_export",
        reason=reason,
        canonical_store_health=canonical_store_health,
        recovery_command="dev map query <symbol>",
        detail=detail
        or (
            "compatibility JSON exceeded indexing.graph_json_max_bytes; "
            "SQLite remains canonical — run `dev map` to refresh exports or "
            "raise graph_json_max_bytes in config"
        ),
    )


def embedding_scan_limit(
    *,
    canonical_store_health: str,
    reason: str,
    detail: Optional[str] = None,
) -> GraphLimitDegraded:
    return GraphLimitDegraded(
        kind="embedding_scan",
        reason=reason,
        canonical_store_health=canonical_store_health,
        recovery_command="dev map search '<query>'",
        detail=detail
        or "omit --semantic to use FTS-backed symbol search instead of embedding scan",
    )


def community_detection_limit(
    *,
    canonical_store_health: str,
    reason: str,
    detail: Optional[str] = None,
) -> GraphLimitDegraded:
    return GraphLimitDegraded(
        kind="community_detection",
        reason=reason,
        canonical_store_health=canonical_store_health,
        recovery_command="dev map",
        detail=detail
        or "community labels skipped — graph visualizer falls back to area coloring",
    )


def collect_limit_reports(*reports: Optional[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Return non-empty structured limit dicts in stable order."""
    out: List[Dict[str, Any]] = []
    for report in reports:
        if report and report.get("degraded"):
            out.append(report)
    return out


def _is_file_node(n: GraphNode) -> bool:
    kind = n.kind.value if hasattr(n.kind, "value") else str(n.kind)
    return kind == NodeKind.FILE.value or ("::" not in n.id and not n.name)


def community_label_for_area(graph: CodeGraph, area: str) -> str:
    """Dominant community label among file nodes under ``area`` (generic subsystems)."""
    counts: Counter[str] = Counter()
    prefix = area.replace("\\", "/").rstrip("/") + "/"
    for n in graph.nodes:
        if not _is_file_node(n):
            continue
        path = (n.path or n.id).replace("\\", "/")
        if path == area or path.startswith(prefix):
            if n.community:
                counts[n.community] += 1
    if not counts:
        return ""
    return counts.most_common(1)[0][0]
