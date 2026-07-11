"""Notifications to the operator — a port of the original ``ntfy.sh`` push.

The Coordinator (and only the Coordinator/Director) may reach the operator. When a campaign finishes
— or a task is blocked — a one-line push goes to an `ntfy <https://ntfy.sh>`_
topic so the operator can watch progress from a phone. A missing topic degrades to
a silent no-op; a failed push never crashes a campaign.
"""

from __future__ import annotations

import os
from typing import List, Optional


class Notifier:
    """Best-effort push notifier backed by ntfy.

    Parameters
    ----------
    topic:
        ntfy topic. Falls back to ``$DIRECTOR_NTFY_TOPIC``. Empty → disabled.
    server:
        ntfy server base URL (default the public ``https://ntfy.sh``). Falls back
        to ``$DIRECTOR_NTFY_SERVER``.
    enabled:
        Force-enable/disable; ``None`` means "enabled iff a topic is set".
    """

    def __init__(
        self,
        topic: Optional[str] = None,
        server: Optional[str] = None,
        enabled: Optional[bool] = None,
    ):
        self.topic = topic or os.environ.get("DIRECTOR_NTFY_TOPIC", "")
        self.server = (server or os.environ.get("DIRECTOR_NTFY_SERVER") or "https://ntfy.sh").rstrip("/")
        self.enabled = bool(self.topic) if enabled is None else enabled
        # Records every push (delivered or not) for the dashboard / tests.
        self.sent: List[str] = []

    def notify(
        self,
        message: str,
        title: Optional[str] = None,
        priority: Optional[str] = None,
        tags: Optional[List[str]] = None,
    ) -> bool:
        """Send a push. Returns ``True`` if it left the machine, else ``False``."""
        self.sent.append(message)
        if not self.enabled or not self.topic:
            return False
        try:  # httpx is a DevCouncil dependency; import lazily to keep startup cheap.
            import httpx

            headers = {}
            if title:
                headers["Title"] = title
            if priority:
                headers["Priority"] = priority
            if tags:
                headers["Tags"] = ",".join(tags)
            token = os.environ.get("DIRECTOR_NTFY_TOKEN")
            if token:
                headers["Authorization"] = f"Bearer {token}"
            resp = httpx.post(
                f"{self.server}/{self.topic}",
                content=message.encode("utf-8"),
                headers=headers,
                timeout=5.0,
            )
            return resp.status_code < 400
        except Exception:
            # Never let a notification failure abort the campaign.
            return False


class NullNotifier(Notifier):
    """A notifier that records messages but never sends — the default."""

    def __init__(self) -> None:
        super().__init__(topic="", enabled=False)

    def notify(
        self,
        message: str,
        title: Optional[str] = None,
        priority: Optional[str] = None,
        tags: Optional[List[str]] = None,
    ) -> bool:
        self.sent.append(message)
        return False
