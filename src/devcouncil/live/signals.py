from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from pydantic import BaseModel, Field


class ReviewSignal(BaseModel):
    client: str = "generic"
    payload: dict[str, Any] = Field(default_factory=dict)
    transcript_path: str | None = None
    session_id: str | None = None
    task_id: str | None = None
    created_at: str = Field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
    review_command: str | None = None
    path: str | None = None


def signal_dir(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "live" / "signals"


def processed_signal_dir(project_root: Path) -> Path:
    return signal_dir(project_root) / "processed"


def write_signal(project_root: Path, client: str, payload: dict[str, Any]) -> Path:
    directory = signal_dir(project_root)
    directory.mkdir(parents=True, exist_ok=True)
    transcript_path = extract_transcript_path(payload)
    session_id = _string_value(payload, "session_id", "sessionId", "session", "cwd")
    task_id = extract_task_id(payload)
    signal = ReviewSignal(
        client=client.lower(),
        payload=payload,
        transcript_path=transcript_path,
        session_id=session_id,
        task_id=task_id,
        review_command=_review_command(client.lower(), transcript_path, task_id),
    )
    key = transcript_path or session_id or json.dumps(payload, sort_keys=True, default=str)
    import hashlib

    digest = hashlib.sha1(key.encode("utf-8", errors="replace")).hexdigest()[:12]
    path = directory / f"{client.lower()}-{digest}.json"
    signal.path = str(path)
    path.write_text(signal.model_dump_json(indent=2) + "\n", encoding="utf-8")
    return path


def load_signals(project_root: Path) -> list[ReviewSignal]:
    directory = signal_dir(project_root)
    if not directory.exists():
        return []
    signals: list[ReviewSignal] = []
    for path in sorted(directory.glob("*.json"), key=lambda item: item.stat().st_mtime, reverse=True):
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
            signal = ReviewSignal.model_validate(raw)
            signal.path = str(path)
            signals.append(signal)
        except Exception:
            continue
    return signals


def mark_processed(signal: ReviewSignal, project_root: Path) -> Path | None:
    if not signal.path:
        return None
    source = Path(signal.path)
    if not source.exists():
        return None
    target_dir = processed_signal_dir(project_root)
    target_dir.mkdir(parents=True, exist_ok=True)
    target = target_dir / source.name
    if target.exists():
        target.unlink()
    source.replace(target)
    return target


def extract_transcript_path(payload: dict[str, Any]) -> str | None:
    direct = _string_value(
        payload,
        "transcript_path",
        "transcriptPath",
        "transcript",
        "conversation_path",
        "conversationPath",
        "file",
        "path",
    )
    if direct:
        return direct
    for key in ("session", "message", "hook_event", "event"):
        nested = payload.get(key)
        if isinstance(nested, dict):
            value = extract_transcript_path(nested)
            if value:
                return value
    return None


def extract_task_id(payload: dict[str, Any]) -> str | None:
    direct = _string_value(payload, "task_id", "taskId", "task", "active_task", "activeTask")
    if direct:
        return direct
    for key in ("session", "message", "hook_event", "event", "metadata"):
        nested = payload.get(key)
        if isinstance(nested, dict):
            value = extract_task_id(nested)
            if value:
                return value
    return None


def _string_value(payload: dict[str, Any], *keys: str) -> str | None:
    for key in keys:
        value = payload.get(key)
        if isinstance(value, str) and value.strip():
            return value
    return None


def _review_command(client: str, transcript_path: str | None, task_id: str | None = None) -> str:
    if transcript_path:
        command = f"dev watch review --client {client} --transcript {transcript_path}"
    else:
        command = f"dev watch pending --client {client}"
    if task_id:
        command += f" --task-id {task_id}"
    return command
