"""Deterministic gap IDs and stable ordering for verification runs."""

from __future__ import annotations

import hashlib
import re
from typing import Iterable, List

from devcouncil.domain.gap import Gap

_SEVERITY_ORDER = {"critical": 0, "high": 1, "medium": 2, "low": 3}


def stable_gap_id(task_id: str, kind: str, identity: str = "") -> str:
    """Return a gap id that is stable across verify runs for the same finding."""
    key = f"{task_id}|{kind}|{identity or kind}"
    digest = hashlib.sha256(key.encode("utf-8")).hexdigest()[:10]
    safe_kind = re.sub(r"[^A-Za-z0-9]", "", kind)[:16] or "GAP"
    return f"GAP-{task_id}-{safe_kind}-{digest}"


def gap_identity(gap: Gap) -> str:
    """Stable dedup key for a gap (type + location + criterion + description)."""
    return "|".join(
        (
            gap.gap_type,
            gap.file or "",
            str(gap.line or ""),
            gap.acceptance_criterion_id or "",
            gap.description.strip(),
        )
    )


def normalize_verify_gaps(gaps: Iterable[Gap]) -> List[Gap]:
    """Dedupe and sort gaps so persistence and reconnecting agents stay stable."""
    seen: dict[str, Gap] = {}
    for gap in gaps:
        key = gap_identity(gap)
        existing = seen.get(key)
        if existing is None:
            seen[key] = gap
            continue
        # Prefer blocking / higher severity when duplicates collide on identity.
        if gap.blocking and not existing.blocking:
            seen[key] = gap
        elif gap.blocking == existing.blocking:
            sev_new = _SEVERITY_ORDER.get(gap.severity, 9)
            sev_old = _SEVERITY_ORDER.get(existing.severity, 9)
            if sev_new < sev_old:
                seen[key] = gap
    return sorted(
        seen.values(),
        key=lambda g: (
            not g.blocking,
            _SEVERITY_ORDER.get(g.severity, 9),
            g.gap_type,
            g.file or "",
            g.line or 0,
            g.acceptance_criterion_id or "",
            g.id,
        ),
    )
