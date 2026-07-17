"""Compaction survival helpers for Claude Code lifecycle hooks.

Persists a pre-compaction snapshot and rebuilds slim post-compaction context from
snapshot, DB, or transcript — without re-injecting the full session briefing.
"""

from __future__ import annotations

import json
import os
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from devcouncil.app.project_status import compute_phase
from devcouncil.live.tasks import active_task_id
from devcouncil.live.transcripts import discover_sessions, latest_assistant_turn
from devcouncil.storage.db import get_db
from devcouncil.telemetry.traces import read_trace_events

COMPACT_SNAPSHOT_REL = Path(".devcouncil") / "state" / "compact_snapshot.json"
_STOP_GATE_EVENT_TYPES = frozenset({"agent_response_ready", "subagent_stop", "post_task_verified"})


def compact_snapshot_path(project_root: Path) -> Path:
    return project_root / COMPACT_SNAPSHOT_REL


def write_json(path: Path, data: dict[str, Any]) -> None:
    """Atomically write JSON to ``path`` via a same-directory temp file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def read_compact_snapshot(project_root: Path) -> dict[str, Any] | None:
    path = compact_snapshot_path(project_root)
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else None
    except Exception:
        return None


def compact_snapshot_recent(project_root: Path, within_seconds: int) -> bool:
    """True when a compact snapshot was written within ``within_seconds``."""
    if within_seconds <= 0:
        return False
    snapshot = read_compact_snapshot(project_root)
    if not snapshot:
        return False
    written_at = snapshot.get("written_at")
    if not isinstance(written_at, str):
        return False
    try:
        written = datetime.fromisoformat(written_at.replace("Z", "+00:00"))
        if written.tzinfo is None:
            written = written.replace(tzinfo=timezone.utc)
        age = (datetime.now(timezone.utc) - written).total_seconds()
        return 0 <= age <= within_seconds
    except Exception:
        return False


def _status_line(root: Path) -> str | None:
    try:
        db = get_db(root)
        if not db:
            return None
        from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository

        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
            summary = graph.coverage_summary()
            state = StateRepository(session).get_state()
            phase = compute_phase(graph, state.current_phase if state else None)
        return (
            f"DevCouncil — phase: {phase}; tasks: {summary['total_tasks']}; "
            f"gaps: {summary['total_gaps']} ({summary['blocking_gaps']} blocking). "
            "Use the devcouncil_* MCP tools and `dev` CLI to stay inside the verify loop."
        )
    except Exception:
        return None


def _phase_and_blocking_from_db(root: Path) -> tuple[str | None, str | None]:
    try:
        db = get_db(root)
        if not db:
            return None, None
        from devcouncil.storage.repositories import ArtifactGraphRepository, GapRepository, StateRepository

        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
            state = StateRepository(session).get_state()
            phase = compute_phase(graph, state.current_phase if state else None)
            blocking = [g for g in GapRepository(session).get_all() if g.blocking]
        return phase, _blocking_gaps_summary(blocking)
    except Exception:
        return None, None


def _blocking_gaps_summary(blocking: list[Any]) -> str | None:
    if not blocking:
        return None
    parts: list[str] = []
    for gap in blocking[:5]:
        label = getattr(gap, "description", None) or str(gap)
        parts.append(str(label).split("\n", 1)[0][:120])
    suffix = f" (+{len(blocking) - 5} more)" if len(blocking) > 5 else ""
    return f"{len(blocking)} blocking gap(s): {'; '.join(parts)}{suffix}"


def _last_stop_gate_event(root: Path) -> dict[str, Any] | None:
    try:
        for event in reversed(list(read_trace_events(root))):
            if event.type in _STOP_GATE_EVENT_TYPES:
                return {
                    "type": event.type,
                    "summary": event.summary,
                    "timestamp": event.timestamp,
                    "task_id": event.task_id,
                }
    except Exception:
        pass
    return None


def _last_assistant_sentence(root: Path, session_id: str | None = None) -> str | None:
    try:
        sessions = discover_sessions(root, client="claude")
        if session_id:
            sessions = [s for s in sessions if s.id == session_id] or sessions
        for session in sessions:
            path = Path(session.transcript_path)
            turn = latest_assistant_turn(path, client="claude")
            if turn and turn.content.strip():
                return _last_sentence(turn.content)
    except Exception:
        pass
    return None


def _last_sentence(text: str) -> str:
    cleaned = text.strip()
    if not cleaned:
        return ""
    parts = re.split(r"(?<=[.!?])\s+", cleaned)
    sentence = parts[-1].strip() if parts else cleaned
    return sentence[:240]


def build_compact_snapshot(root: Path, payload: dict[str, Any]) -> dict[str, Any]:
    session_id = payload.get("session_id") or payload.get("sessionId")
    if session_id is not None:
        session_id = str(session_id)
    task_id = active_task_id(root)
    phase, blocking_summary = _phase_and_blocking_from_db(root)
    return {
        "written_at": datetime.now(timezone.utc).isoformat(),
        "session_id": session_id,
        "task_id": task_id,
        "phase": phase,
        "blocking_gaps_summary": blocking_summary,
        "last_stop_gate_event": _last_stop_gate_event(root),
        "last_assistant_sentence": _last_assistant_sentence(root, session_id),
    }


def _briefing_from_snapshot(snapshot: dict[str, Any]) -> str:
    lines = ["DevCouncil (post-compaction recovery):"]
    task_id = snapshot.get("task_id")
    if task_id:
        lines.append(f"Active task: {task_id}")
    phase = snapshot.get("phase")
    if phase:
        lines.append(f"Phase: {phase}")
    blocking = snapshot.get("blocking_gaps_summary")
    if blocking:
        lines.append(str(blocking))
    stop_event = snapshot.get("last_stop_gate_event")
    if isinstance(stop_event, dict) and stop_event.get("summary"):
        lines.append(f"Last stop gate: {stop_event['summary']}")
    last_sentence = snapshot.get("last_assistant_sentence")
    if last_sentence:
        lines.append(f"Last assistant context: {last_sentence}")
    lines.append("Use devcouncil_* MCP tools and `dev` CLI to continue the verify loop.")
    return "\n".join(lines)


def _briefing_from_db(root: Path) -> str | None:
    status = _status_line(root)
    if not status:
        return None
    lines = ["DevCouncil (post-compaction recovery):", status.split(". Use the")[0]]
    task_id = active_task_id(root)
    if task_id:
        lines.append(f"Active task: {task_id}")
    _, blocking = _phase_and_blocking_from_db(root)
    if blocking:
        lines.append(blocking)
    return "\n".join(lines)


def _briefing_from_transcript(root: Path, session_id: str | None = None) -> str | None:
    sentence = _last_assistant_sentence(root, session_id)
    if not sentence:
        return None
    return f"DevCouncil (post-compaction recovery): Last assistant context: {sentence}"


def session_briefing(root: Path) -> str | None:
    """Full status + active-task context for normal SessionStart events."""
    status = _status_line(root)
    if not status:
        return None
    lines = [status]
    task_id = active_task_id(root)
    if task_id:
        lines.append(f"Active task: {task_id}")
    _, blocking = _phase_and_blocking_from_db(root)
    if blocking:
        lines.append(blocking)
    return "\n".join(lines)


def compact_briefing(root: Path, payload: dict[str, Any] | None = None) -> str | None:
    """Slim post-compaction briefing — prefer snapshot, then DB, then transcript."""
    payload = payload or {}
    session_id = payload.get("session_id") or payload.get("sessionId")
    if session_id is not None:
        session_id = str(session_id)

    snapshot = read_compact_snapshot(root)
    if snapshot:
        return _briefing_from_snapshot(snapshot)

    db_brief = _briefing_from_db(root)
    if db_brief:
        return db_brief

    return _briefing_from_transcript(root, session_id)
