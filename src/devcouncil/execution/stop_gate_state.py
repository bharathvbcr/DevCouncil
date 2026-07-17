"""Per-session block-count state under ``.devcouncil/state/stop_gate_blocks.json``."""

from __future__ import annotations

import json
from pathlib import Path

from devcouncil.utils.json_persist import read_json, write_json

STATE_REL = Path(".devcouncil") / "state" / "stop_gate_blocks.json"


def state_path(project_root: Path) -> Path:
    return project_root / STATE_REL


def get_block_count(project_root: Path, session_id: str) -> int:
    if not session_id:
        return 0
    try:
        data = read_json(state_path(project_root)) or {}
        if not isinstance(data, dict):
            return 0
        sessions = data.get("sessions") or {}
        if not isinstance(sessions, dict):
            return 0
        count = sessions.get(session_id, 0)
        return count if isinstance(count, int) and count >= 0 else 0
    except (OSError, json.JSONDecodeError, TypeError, AttributeError):
        return 0


def increment_block_count(project_root: Path, session_id: str) -> int:
    if not session_id:
        return 0
    path = state_path(project_root)
    try:
        try:
            data = read_json(path) or {}
        except FileNotFoundError:
            data = {}
        if not isinstance(data, dict):
            data = {}
        sessions = data.setdefault("sessions", {})
        if not isinstance(sessions, dict):
            sessions = {}
            data["sessions"] = sessions
        count = get_block_count(project_root, session_id) + 1
        sessions[session_id] = count
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, data)
        return count
    except (OSError, TypeError, ValueError):
        return get_block_count(project_root, session_id) + 1
