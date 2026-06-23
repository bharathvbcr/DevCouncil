import json
import logging
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Tuple

from pydantic import BaseModel, ConfigDict, Field

logger = logging.getLogger(__name__)

TRACE_SCHEMA_VERSION = "devcouncil.trace.v1"


class TraceEvent(BaseModel):
    """Stable JSONL event shape for DevCouncil and external visualizers."""

    model_config = ConfigDict(populate_by_name=True, serialize_by_alias=True)

    schema_version: str = Field(TRACE_SCHEMA_VERSION, alias="schema")
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    timestamp: str = Field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
    runtime: str = "devcouncil"
    type: str
    run_id: Optional[str] = None
    task_id: Optional[str] = None
    summary: str = ""
    details: Dict[str, Any] = Field(default_factory=dict)

    @classmethod
    def from_legacy(cls, raw: Dict[str, Any]) -> "TraceEvent":
        if raw.get("schema") == TRACE_SCHEMA_VERSION:
            return cls.model_validate(raw)
        raw_details = raw.get("details")
        details: Dict[str, Any] = raw_details if isinstance(raw_details, dict) else {}
        raw_task_id = details.get("task_id")
        raw_summary = details.get("summary")
        raw_run_id = raw.get("run_id")
        return cls(
            schema=TRACE_SCHEMA_VERSION,
            type=str(raw.get("type", "legacy_event")),
            run_id=raw_run_id if isinstance(raw_run_id, str) else None,
            task_id=raw_task_id if isinstance(raw_task_id, str) else None,
            summary=raw_summary if isinstance(raw_summary, str) else "",
            details=details,
        )


class TraceLogger:
    """Manages appending execution traces to a local log file."""

    def __init__(self, project_root: Path):
        self.log_dir = project_root / ".devcouncil" / "logs"
        self.trace_file = self.log_dir / "traces.jsonl"
        self.log_dir.mkdir(parents=True, exist_ok=True)

    def log_event(
        self,
        event_type: str,
        details: Dict[str, Any],
        run_id: Optional[str] = None,
        task_id: Optional[str] = None,
        summary: str = "",
    ) -> TraceEvent:
        """Append an orchestration event trace."""
        trace = TraceEvent(
            schema=TRACE_SCHEMA_VERSION,
            type=event_type,
            details=details,
            run_id=run_id,
            task_id=task_id,
            summary=summary,
        )
        try:
            with open(self.trace_file, "a", encoding="utf-8") as f:
                f.write(trace.model_dump_json() + "\n")
        except Exception as e:
            logger.debug("Failed to write trace event %s: %s", event_type, e)
        return trace


def read_trace_events(project_root: Path) -> Iterable[TraceEvent]:
    trace_file = project_root / ".devcouncil" / "logs" / "traces.jsonl"
    if not trace_file.exists():
        return []

    events: list[TraceEvent] = []
    for line in trace_file.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            events.append(TraceEvent.from_legacy(json.loads(line)))
        except Exception as exc:
            logger.debug("Skipping invalid trace line: %s", exc)
    return events


def read_trace_events_since(
    project_root: Path, cursor: Optional[int] = None
) -> Tuple[List[TraceEvent], int]:
    """Incrementally read trace events appended after ``cursor``.

    The cursor is a byte offset into the trace file, which is robust to appends
    (the file is append-only) and makes repeated polling O(new bytes) rather than
    O(all events). Returns ``(events, next_cursor)`` where ``next_cursor`` should
    be passed back on the next call to fetch only newer events.

    Never raises: on any error it returns an empty batch and a safe cursor. A
    ``cursor`` past the current end-of-file (e.g. the log was rotated/truncated)
    is treated as a reset so the caller still makes progress.
    """
    trace_file = project_root / ".devcouncil" / "logs" / "traces.jsonl"
    start = cursor if isinstance(cursor, int) and cursor >= 0 else 0
    if not trace_file.exists():
        return [], start

    try:
        size = trace_file.stat().st_size
        # File shrank (rotation/truncation) — restart from the beginning.
        if start > size:
            start = 0

        with open(trace_file, "rb") as f:
            f.seek(start)
            chunk = f.read()
    except Exception as exc:
        logger.debug("Failed incremental trace read: %s", exc)
        return [], start

    # Only consume up to the last complete line so a half-written final line is
    # re-read (not skipped) on the next poll once it is fully flushed.
    last_newline = chunk.rfind(b"\n")
    if last_newline == -1:
        return [], start
    consumed = chunk[: last_newline + 1]
    next_cursor = start + len(consumed)

    events: List[TraceEvent] = []
    for raw_line in consumed.decode("utf-8", errors="replace").splitlines():
        if not raw_line.strip():
            continue
        try:
            events.append(TraceEvent.from_legacy(json.loads(raw_line)))
        except Exception as exc:
            logger.debug("Skipping invalid trace line: %s", exc)
    return events, next_cursor
