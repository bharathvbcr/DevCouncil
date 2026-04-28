import json
import logging
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, Optional

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
        details = raw.get("details") if isinstance(raw.get("details"), dict) else {}
        task_id = details.get("task_id") if isinstance(details.get("task_id"), str) else None
        summary = details.get("summary") if isinstance(details.get("summary"), str) else ""
        return cls(
            type=str(raw.get("type", "legacy_event")),
            run_id=raw.get("run_id"),
            task_id=task_id,
            summary=summary,
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
