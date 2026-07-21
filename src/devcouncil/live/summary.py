from __future__ import annotations

from pathlib import Path

from devcouncil.live.cards import load_cards, unresolved_blocking_cards
from devcouncil.live.signals import ReviewSignal, load_signals
from devcouncil.live.tasks import active_task_id


def _compact_signal_item(signal: ReviewSignal) -> dict:
    """IDs/counts-safe projection for general status (no PII / absolute paths)."""
    signal_id = Path(signal.path).name if signal.path else None
    return {
        "id": signal_id,
        "client": signal.client,
        "task_id": signal.task_id,
    }


def live_review_summary(
    project_root: Path,
    task_id: str | None = None,
    *,
    include_signal_details: bool = False,
) -> dict:
    cards = load_cards(project_root)
    signals = load_signals(project_root)
    active_id = active_task_id(project_root)
    scoped_task_id = task_id or active_id
    blockers = unresolved_blocking_cards(project_root, task_id=scoped_task_id, cards=cards)
    if include_signal_details:
        pending_signal_items = [signal.model_dump() for signal in signals]
    else:
        pending_signal_items = [_compact_signal_item(signal) for signal in signals]
    open_count = 0
    resolved_count = 0
    ignored_count = 0
    critical_open_count = 0
    for card in cards:
        if card.status == "open":
            open_count += 1
            if card.verdict == "Critical Issues":
                critical_open_count += 1
        elif card.status == "resolved":
            resolved_count += 1
        elif card.status == "ignored":
            ignored_count += 1
    return {
        "active_task_id": active_id,
        "scope_task_id": scoped_task_id,
        "pending_signals": len(signals),
        "pending_signal_items": pending_signal_items[:10],
        "cards": {
            "total": len(cards),
            "open": open_count,
            "resolved": resolved_count,
            "ignored": ignored_count,
            "critical_open": critical_open_count,
        },
        "blocking_cards": [card.model_dump() for card in blockers],
        "recent_cards": [card.model_dump() for card in cards[:5]],
    }
