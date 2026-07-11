"""Mailbox watcher — the "you have mail" nudge layer.

In the original, one ``inotifywait``/``fswatch`` daemon per agent blocks on a
kernel file-change event and, when the inbox changes, fires a content-free
``inboxN`` keystroke into that agent's tmux pane. Message *content* never travels
over the wire — only the wake-up.

DevCouncil runs its agents in-process, so the "keystroke" becomes a Python
callback: ``nudge(agent, unread_count)``. We still key off real filesystem
events (via :mod:`watchdog`, already a DevCouncil dependency) so cross-process
writers — e.g. a leased MCP agent appending to a mailbox — also wake the
campaign. When watchdog is unavailable we fall back to a polling thread, and
:meth:`MailboxWatcher.poll_once` lets a single-threaded orchestrator drive the
same logic deterministically (and keeps unit tests free of timing races).
"""

from __future__ import annotations

import threading
from typing import Any, Callable, Dict, Iterable, List, Optional

from devcouncil.campaign.mailbox import Mailbox

NudgeCallback = Callable[[str, int], None]

try:  # watchdog is a declared dependency; degrade gracefully if it is missing.
    from watchdog.events import FileSystemEventHandler
    from watchdog.observers import Observer

    _HAS_WATCHDOG = True
except Exception:  # pragma: no cover - exercised only without watchdog
    FileSystemEventHandler = object  # type: ignore[assignment,misc]
    Observer = None  # type: ignore[assignment]
    _HAS_WATCHDOG = False


class MailboxWatcher:
    """Watch a set of agent mailboxes and nudge on new unread mail.

    Parameters
    ----------
    mailbox:
        The shared :class:`~devcouncil.campaign.mailbox.Mailbox`.
    agents:
        Agent ids to watch.
    nudge:
        Called ``nudge(agent, unread_count)`` whenever an agent's unread count
        rises. Never called with a count of zero.
    poll_interval:
        Seconds between scans on the polling fallback.
    """

    def __init__(
        self,
        mailbox: Mailbox,
        agents: Iterable[str],
        nudge: NudgeCallback,
        poll_interval: float = 1.0,
    ):
        self.mailbox = mailbox
        self.agents: List[str] = list(agents)
        self.nudge = nudge
        self.poll_interval = poll_interval
        # Last unread count we nudged for, per agent — so a nudge fires on the
        # *rising edge* only (0->N or N->N+1), never repeatedly for the same mail.
        self._last: Dict[str, int] = {a: 0 for a in self.agents}
        self._lock = threading.Lock()
        self._observer: Any = None
        self._stop = threading.Event()
        self._poll_thread: Optional[threading.Thread] = None

    # -- core logic ------------------------------------------------------------

    def poll_once(self) -> List[str]:
        """Scan all mailboxes once; nudge agents whose unread count rose.

        Returns the list of agents nudged. Safe to call from any thread and from
        tests; this is the single choke-point through which every nudge flows.
        """
        nudged: List[str] = []
        with self._lock:
            for agent in self.agents:
                count = self.mailbox.count_unread(agent)
                previous = self._last.get(agent, 0)
                if count > previous and count > 0:
                    self._last[agent] = count
                    nudged.append(agent)
                elif count < previous:
                    # Agent drained its inbox; re-arm so the next arrival nudges.
                    self._last[agent] = count
        for agent in nudged:
            self.nudge(agent, self.mailbox.count_unread(agent))
        return nudged

    def reset(self, agent: Optional[str] = None) -> None:
        """Forget prior nudge state so the next scan re-nudges pending mail."""
        with self._lock:
            if agent is None:
                self._last = {a: 0 for a in self.agents}
            else:
                self._last[agent] = 0

    def add_agent(self, agent: str) -> None:
        with self._lock:
            if agent not in self.agents:
                self.agents.append(agent)
                self._last[agent] = 0

    # -- background operation --------------------------------------------------

    def start(self) -> None:
        """Begin watching in the background (watchdog, or a polling thread)."""
        self.mailbox.inbox_dir.mkdir(parents=True, exist_ok=True)
        if _HAS_WATCHDOG and Observer is not None:
            handler = _InboxEventHandler(self)
            self._observer = Observer()
            self._observer.schedule(handler, str(self.mailbox.inbox_dir), recursive=False)
            self._observer.start()
        else:  # pragma: no cover - fallback path
            self._poll_thread = threading.Thread(target=self._poll_loop, daemon=True)
            self._poll_thread.start()
        # Deliver anything already waiting at startup.
        self.poll_once()

    def _poll_loop(self) -> None:  # pragma: no cover - timing loop
        while not self._stop.wait(self.poll_interval):
            self.poll_once()

    def stop(self) -> None:
        self._stop.set()
        if self._observer is not None:
            try:
                self._observer.stop()
                self._observer.join(timeout=2.0)
            except Exception:
                pass
            self._observer = None
        if self._poll_thread is not None:  # pragma: no cover
            self._poll_thread.join(timeout=2.0)
            self._poll_thread = None

    def __enter__(self) -> "MailboxWatcher":
        self.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self.stop()


class _InboxEventHandler(FileSystemEventHandler):
    """Translates filesystem change events into a debounced ``poll_once``."""

    def __init__(self, watcher: MailboxWatcher):
        self._watcher = watcher

    def on_any_event(self, event: object) -> None:  # pragma: no cover - needs fs events
        # A YAML write shows up as create/modify/moved on the atomic replace; we
        # do not care which — a single re-scan reconciles all mailboxes.
        self._watcher.poll_once()
