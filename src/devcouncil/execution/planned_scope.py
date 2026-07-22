"""Shared planned-file path matching for CLI and MCP surfaces.

CLI gated writes historically accepted exact paths and ``fnmatch`` globs (and
stripped a leading ``./``). MCP task-scoped reads/diffs used exact set
membership only, which produced observable allow/deny drift for the same task
scope. Callers must use this helper so both surfaces stay aligned.
"""

from __future__ import annotations

import fnmatch
from collections.abc import Iterable
from typing import Any


def _planned_path(entry: Any) -> str:
    raw = entry.path if hasattr(entry, "path") else entry
    return str(raw).replace("\\", "/")


def normalize_planned_candidate(path: str) -> str:
    """Normalize a candidate path the way write authorization does."""
    normalized = path.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return normalized


def matches_planned_path(path: str, planned_files: Iterable[Any]) -> bool:
    """Return True when ``path`` matches a planned file exactly or via glob."""
    normalized = normalize_planned_candidate(path)
    return any(
        normalized == planned or fnmatch.fnmatch(normalized, planned)
        for planned in (_planned_path(entry) for entry in planned_files)
    )
