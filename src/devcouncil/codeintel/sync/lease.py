"""Portable best-effort cross-process writer lease."""

from __future__ import annotations

import os
import time
from pathlib import Path
from typing import BinaryIO, Callable


class WriterLease:
    def __init__(self, path: Path):
        self.path = path
        self._handle: BinaryIO | None = None

    def acquire(self) -> bool:
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
