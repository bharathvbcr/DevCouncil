from __future__ import annotations

from pathlib import Path

from devcouncil.live.cards import load_cards, unresolved_blocking_cards
from devcouncil.live.signals import load_signals
from devcouncil.live.tasks import active_task_id


def live_review_summary(project_root: Path, task_id: str | None = None) -> dict:
    cards = load_cards(project_root)
    signals = load_signals(project_root)
    active_id = active_task_id(project_root)
    scoped_task_id = task_id or active_id
    blockers = unresolved_blocking_cards(project_root, task_id=scoped_task_id)
    pending_signal_items = [signal.model_dump() for signal in signals]
    return {
        "active_task_id": active_id,
        "scope_task_id": scoped_task_id,
        "pending_signals": len(signals),
        "pending_signal_items": pending_signal_items[:10],
        "cards": {
            "total": len(cards),
            "open": len([card for card in cards if card.status == "open"]),
            "resolved": len([card for card in cards if card.status == "resolved"]),
            "ignored": len([card for card in cards if card.status == "ignored"]),
            "critical_open": len([
                card for card in cards
                if card.status == "open" and card.verdict == "Critical Issues"
            ]),
        },
        "blocking_cards": [card.model_dump() for card in blockers],
        "recent_cards": [card.model_dump() for card in cards[:5]],
    }
