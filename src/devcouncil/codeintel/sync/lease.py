"""Portable best-effort cross-process writer lease."""

from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Callable


@dataclass(frozen=True)
class LeaseHolder:
    """Best-effort identity written into the lock file after flock succeed.

    ``flock`` / ``msvcrt.locking`` remains the source of truth for ownership;
    metadata is advisory for ``dev map unlock`` and status surfaces.
    """

    pid: int | None = None
    started_at: float | None = None


def read_holder(path: Path) -> LeaseHolder:
    """Read advisory holder metadata from ``path`` without taking the lease."""
    try:
        raw = path.read_bytes()
    except OSError:
        return LeaseHolder()
    text = raw.decode("utf-8", errors="replace").strip("\0").strip()
    if not text:
        return LeaseHolder()
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        return LeaseHolder()
    if not isinstance(data, dict):
        return LeaseHolder()
    pid_raw = data.get("pid")
    started_raw = data.get("started_at")
    try:
        pid = int(pid_raw) if pid_raw is not None else None
    except (TypeError, ValueError):
        pid = None
    try:
        started_at = float(started_raw) if started_raw is not None else None
    except (TypeError, ValueError):
        started_at = None
    return LeaseHolder(pid=pid, started_at=started_at)


class WriterLease:
    def __init__(self, path: Path):
        self.path = path
        self._handle: BinaryIO | None = None

    def acquire(self) -> bool:
        # Re-entrant acquire would leak the prior fd while still holding flock (#14).
        if self._handle is not None:
            self.release()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        handle = self.path.open("a+b")
        try:
            if os.name == "nt":
                import msvcrt

                handle.seek(0)
                if handle.tell() == 0:
                    handle.write(b"\0")
                    handle.flush()
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)  # type: ignore[attr-defined]
            else:
                import fcntl

                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            self._write_holder_metadata(handle)
        except (OSError, BlockingIOError):
            handle.close()
            return False
        self._handle = handle
        return True

    def acquire_with_retry(
        self,
        *,
        timeout: float = 30.0,
        initial_delay: float = 0.05,
        max_delay: float = 1.0,
        sleep: Callable[[float], None] | None = None,
    ) -> bool:
        """Acquire the lease, retrying with exponential backoff until ``timeout``.

        Concurrent watchers / MCP writers commonly hold ``writer.lock`` for a short
        window. Callers that would otherwise fail closed on the first busy probe
        should use this so pending work drains instead of oscillating into
        ``read_only`` / lean-map fallbacks.
        """
        sleeper = sleep or time.sleep
        if self.acquire():
            return True
        deadline = time.monotonic() + max(0.0, timeout)
        delay = max(0.0, initial_delay)
        max_delay = max(delay, max_delay)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            sleeper(min(delay, remaining, max_delay))
            if self.acquire():
                return True
            if delay <= 0:
                delay = max(initial_delay, 0.05)
            else:
                delay = min(max_delay, delay * 2)

    def release(self) -> None:
        handle = self._handle
        if handle is None:
            return
        try:
            if os.name == "nt":
                import msvcrt

                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)  # type: ignore[attr-defined]
            else:
                import fcntl

                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        finally:
            handle.close()
            self._handle = None

    def __enter__(self) -> "WriterLease":
        if not self.acquire():
            raise BlockingIOError("another code-intelligence writer owns the project lease")
        return self

    def __exit__(self, *_args: object) -> None:
        self.release()

    @staticmethod
    def _write_holder_metadata(handle: BinaryIO) -> None:
        payload = json.dumps(
            {"pid": os.getpid(), "started_at": time.time()},
            separators=(",", ":"),
        ).encode("utf-8")
        handle.seek(0)
        handle.truncate()
        handle.write(payload)
        handle.flush()
