"""Short-lived cache of active-task verify outcomes for the stop gate.

When MCP ``verify_task`` / ``verify-leased`` finishes, the result is recorded so
the Stop hook can skip a full re-verify if the cache is still fresh.
"""

from __future__ import annotations

import time
from pathlib import Path
from typing import Any

from devcouncil.utils.json_persist import read_json, write_json

CACHE_REL = Path(".devcouncil") / "cache" / "stop_gate_verify.json"
DEFAULT_TTL_MINUTES = 5


def cache_path(project_root: Path) -> Path:
    return project_root / CACHE_REL


def record_verify_cache(
    project_root: Path,
    *,
    task_id: str,
    status: str,
    blocking_gaps: int,
    next_actions: list[dict[str, Any]] | None = None,
    passed: bool | None = None,
) -> None:
    try:
        payload = {
            "task_id": task_id,
            "status": status,
            "blocking_gaps": blocking_gaps,
            "passed": passed if passed is not None else blocking_gaps == 0,
            "next_actions": (next_actions or [])[:20],
            "updated_at": time.time(),
        }
        path = cache_path(project_root)
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, payload)
    except (OSError, TypeError, ValueError):
        pass


def load_verify_cache(
    project_root: Path,
    *,
    task_id: str,
    ttl_minutes: int = DEFAULT_TTL_MINUTES,
) -> dict[str, Any] | None:
    if ttl_minutes <= 0:
        return None
    try:
        data = read_json(cache_path(project_root))
    except (OSError, TypeError, ValueError):
        return None
    if not isinstance(data, dict):
        return None
    if data.get("task_id") != task_id:
        return None
    updated = data.get("updated_at")
    if not isinstance(updated, (int, float)):
        return None
    if time.time() - float(updated) > ttl_minutes * 60:
        return None
    return data
