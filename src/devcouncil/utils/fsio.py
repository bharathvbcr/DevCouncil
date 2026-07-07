"""Atomic file-write helpers.

State files (checkpoints, handoff manifests, semantic indexes, OKF bundles)
must never be left half-written by a crash or SIGKILL mid-write. These helpers
write to a temp file in the same directory and ``os.replace`` it into place,
which is atomic on POSIX and Windows (same volume).
"""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any, Union

PathLike = Union[str, Path]


def atomic_write_text(path: PathLike, data: str, *, encoding: str = "utf-8") -> None:
    """Atomically replace ``path`` with ``data``. Parent dir must exist."""
    target = Path(path)
    fd, tmp_name = tempfile.mkstemp(dir=str(target.parent), prefix=f".{target.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding=encoding, newline="") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_name, target)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def atomic_write_bytes(path: PathLike, data: bytes) -> None:
    """Atomically replace ``path`` with binary ``data``. Parent dir must exist."""
    target = Path(path)
    fd, tmp_name = tempfile.mkstemp(dir=str(target.parent), prefix=f".{target.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_name, target)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def atomic_write_json(path: PathLike, payload: Any, *, indent: int = 2, sort_keys: bool = False) -> None:
    """Serialize ``payload`` to JSON and atomically write it to ``path``."""
    atomic_write_text(path, json.dumps(payload, indent=indent, sort_keys=sort_keys, default=str) + "\n")
