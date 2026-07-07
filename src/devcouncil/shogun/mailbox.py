"""File-based agent mailbox — the Shogun message bus.

Faithful port of the original ``scripts/inbox_write.sh`` semantics:

* one YAML file per agent at ``.devcouncil/shogun/inbox/<agent>.yaml`` holding a
  ``messages:`` list of ``{id, from, timestamp, type, content, read}`` entries;
* every append happens under a cross-process lock (an ``mkdir`` spin-mutex, plus
  ``fcntl.flock`` where available) so concurrent Ashigaru never corrupt a file;
* writes are atomic (temp file + :func:`os.replace`) so a reader never observes a
  half-written file;
* the file is capped (all unread + newest ``MAX_READ_RETAINED`` read) so a busy
  campaign cannot grow a mailbox without bound;
* self-sends are rejected (an agent may not mail itself).

Delivery is considered guaranteed the instant the write succeeds — there are no
ACKs or retries. Waking the recipient is a separate concern handled by
:mod:`devcouncil.shogun.watcher`.
"""

from __future__ import annotations

import os
import tempfile
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, List, Optional

import yaml

try:  # POSIX advisory lock; optional, we still hold the mkdir spin-mutex without it.
    import fcntl
except ImportError:  # pragma: no cover - Windows
    fcntl = None  # type: ignore[assignment]

MAX_MESSAGES = 50
"""Hard cap on messages retained per mailbox file."""

MAX_READ_RETAINED = 30
"""When capping, keep every unread message plus this many newest read ones."""

LOCK_TIMEOUT_S = 5.0
"""How long :meth:`Mailbox._lock` spins before giving up on the mutex."""

# Message types the watcher consumes directly (context resets, model swaps, …).
# They are delivered but excluded from the "N unread" nudge count so they never
# masquerade as work waiting for the agent.
SPECIAL_TYPES = frozenset({"clear_command", "model_switch", "cli_restart"})


def _utcnow() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


@dataclass
class Message:
    """One entry in an agent's mailbox."""

    id: str
    from_agent: str
    timestamp: str
    type: str
    content: str
    read: bool = False

    def to_dict(self) -> Dict[str, object]:
        # Persist with the original schema's ``from`` key (a Python keyword, hence
        # the ``from_agent`` attribute name).
        return {
            "id": self.id,
            "from": self.from_agent,
            "timestamp": self.timestamp,
            "type": self.type,
            "content": self.content,
            "read": self.read,
        }

    @classmethod
    def from_dict(cls, raw: Dict[str, object]) -> "Message":
        return cls(
            id=str(raw.get("id", "")),
            from_agent=str(raw.get("from", raw.get("from_agent", ""))),
            timestamp=str(raw.get("timestamp", "")),
            type=str(raw.get("type", "info")),
            content=str(raw.get("content", "")),
            read=bool(raw.get("read", False)),
        )

    @property
    def is_special(self) -> bool:
        return self.type in SPECIAL_TYPES


class MailboxError(RuntimeError):
    """Raised when a mailbox invariant is violated (e.g. a self-send)."""


class Mailbox:
    """Read/write access to every agent's on-disk inbox.

    A single :class:`Mailbox` instance is safe to share across threads; all
    mutation goes through the per-agent file lock.
    """

    def __init__(self, root: Path | str = Path(".")):
        self.root = Path(root)
        self.inbox_dir = self.root / ".devcouncil" / "shogun" / "inbox"

    # -- paths -----------------------------------------------------------------

    def path_for(self, agent: str) -> Path:
        return self.inbox_dir / f"{agent}.yaml"

    def _ensure_dir(self) -> None:
        self.inbox_dir.mkdir(parents=True, exist_ok=True)

    # -- locking ---------------------------------------------------------------

    class _Lock:
        """``with mailbox._lock(agent):`` — mkdir spin-mutex + optional flock."""

        def __init__(self, path: Path):
            self._mutex_dir = path.with_suffix(path.suffix + ".lock.d")
            self._flock_path = path.with_suffix(path.suffix + ".flock")
            self._flock_fd: Optional[int] = None

        def __enter__(self) -> "Mailbox._Lock":
            deadline = time.monotonic() + LOCK_TIMEOUT_S
            self._mutex_dir.parent.mkdir(parents=True, exist_ok=True)
            while True:
                try:
                    self._mutex_dir.mkdir()  # atomic on POSIX + Windows
                    break
                except FileExistsError:
                    if time.monotonic() > deadline:
                        # Stale lock recovery: force through rather than deadlock a
                        # whole campaign on a crashed writer.
                        break
                    time.sleep(0.02)
            if fcntl is not None:
                try:
                    self._flock_fd = os.open(self._flock_path, os.O_CREAT | os.O_RDWR, 0o644)
                    fcntl.flock(self._flock_fd, fcntl.LOCK_EX)
                except OSError:
                    self._flock_fd = None
            return self

        def __exit__(self, *exc: object) -> None:
            if self._flock_fd is not None:
                try:
                    fcntl.flock(self._flock_fd, fcntl.LOCK_UN)  # type: ignore[union-attr]
                    os.close(self._flock_fd)
                except OSError:
                    pass
                self._flock_fd = None
            try:
                self._mutex_dir.rmdir()
            except OSError:
                pass

    def _lock(self, agent: str) -> "Mailbox._Lock":
        self._ensure_dir()
        return Mailbox._Lock(self.path_for(agent))

    # -- io --------------------------------------------------------------------

    def _read_raw(self, agent: str) -> List[Message]:
        path = self.path_for(agent)
        if not path.exists():
            return []
        try:
            data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
        except (yaml.YAMLError, OSError):
            return []
        raw = data.get("messages", []) if isinstance(data, dict) else []
        return [Message.from_dict(m) for m in raw if isinstance(m, dict)]

    def _write_atomic(self, agent: str, messages: List[Message]) -> None:
        self._ensure_dir()
        path = self.path_for(agent)
        payload = {"agent": agent, "messages": [m.to_dict() for m in messages]}
        text = yaml.safe_dump(payload, allow_unicode=True, sort_keys=False)
        fd, tmp = tempfile.mkstemp(dir=str(self.inbox_dir), prefix=f".{agent}.", suffix=".tmp")
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as fh:
                fh.write(text)
                fh.flush()
                os.fsync(fh.fileno())
            os.replace(tmp, path)  # atomic swap
        finally:
            if os.path.exists(tmp):
                os.unlink(tmp)

    @staticmethod
    def _cap(messages: List[Message]) -> List[Message]:
        if len(messages) <= MAX_MESSAGES:
            return messages
        unread = [m for m in messages if not m.read]
        read = [m for m in messages if m.read]
        kept_read = read[-MAX_READ_RETAINED:]
        # Preserve original ordering (oldest→newest) while dropping the oldest read.
        keep = set(id(m) for m in unread) | set(id(m) for m in kept_read)
        return [m for m in messages if id(m) in keep]

    # -- public api ------------------------------------------------------------

    def send(
        self,
        target: str,
        content: str,
        type: str = "info",
        from_agent: str = "shogun",
    ) -> Message:
        """Append a message to ``target``'s mailbox and return it.

        Raises :class:`MailboxError` on a self-send.
        """
        if target == from_agent:
            raise MailboxError(f"{from_agent} may not send mail to itself")
        message = Message(
            id=uuid.uuid4().hex[:12],
            from_agent=from_agent,
            timestamp=_utcnow(),
            type=type,
            content=content,
            read=False,
        )
        with self._lock(target):
            messages = self._read_raw(target)
            messages.append(message)
            self._write_atomic(target, self._cap(messages))
        return message

    def all(self, agent: str) -> List[Message]:
        """Every message currently in ``agent``'s mailbox (oldest→newest)."""
        return self._read_raw(agent)

    def unread(self, agent: str) -> List[Message]:
        """Unread messages in delivery order."""
        return [m for m in self._read_raw(agent) if not m.read]

    def count_unread(self, agent: str, exclude_special: bool = True) -> int:
        """Number of unread messages — the ``N`` in an ``inboxN`` nudge."""
        msgs = self.unread(agent)
        if exclude_special:
            msgs = [m for m in msgs if not m.is_special]
        return len(msgs)

    def mark_read(self, agent: str, ids: Optional[List[str]] = None) -> int:
        """Mark messages read. ``ids=None`` marks all. Returns count changed."""
        target_ids = set(ids) if ids is not None else None
        changed = 0
        with self._lock(agent):
            messages = self._read_raw(agent)
            for m in messages:
                if not m.read and (target_ids is None or m.id in target_ids):
                    m.read = True
                    changed += 1
            if changed:
                self._write_atomic(agent, messages)
        return changed

    def drain(self, agent: str, exclude_special: bool = False) -> List[Message]:
        """Return unread messages and mark them read in one locked pass."""
        with self._lock(agent):
            messages = self._read_raw(agent)
            picked: List[Message] = []
            for m in messages:
                if m.read:
                    continue
                if exclude_special and m.is_special:
                    continue
                m.read = True
                picked.append(m)
            if picked:
                self._write_atomic(agent, messages)
        return picked

    def clear(self, agent: str) -> None:
        """Reset an agent's mailbox (used by campaign ``--clean``)."""
        with self._lock(agent):
            self._write_atomic(agent, [])
