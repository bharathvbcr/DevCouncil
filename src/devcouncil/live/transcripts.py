from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Iterable

from devcouncil.live.models import AgentSession, AgentTurn, session_id_from_path


CLAUDE_TRANSCRIPT_ROOT = Path.home() / ".claude" / "projects"


def discover_sessions(project_root: Path, client: str = "claude") -> list[AgentSession]:
    """Find local coding-agent transcripts DevCouncil can review."""
    client = client.lower()
    if client == "claude":
        candidates = _claude_transcript_candidates(project_root)
    else:
        candidates = sorted((project_root / ".devcouncil" / "live" / client).glob("*.jsonl"))

    sessions: list[AgentSession] = []
    for path in candidates:
        if not path.exists() or not path.is_file():
            continue
        stat = path.stat()
        sessions.append(AgentSession(
            id=session_id_from_path(path),
            client=client,
            transcript_path=str(path),
            updated_at=str(stat.st_mtime),
            turns=sum(1 for _ in _safe_lines(path)),
        ))
    return sorted(sessions, key=lambda item: item.updated_at or "", reverse=True)


def load_turns(path: Path, client: str = "generic") -> list[AgentTurn]:
    """Parse a transcript into normalized turns.

    Supports Claude Code JSONL plus generic JSONL records with role/content fields.
    """
    turns: list[AgentTurn] = []
    session_id = session_id_from_path(path)
    for index, raw in enumerate(_read_jsonl(path)):
        turn = _turn_from_record(raw, session_id=session_id, turn_index=index, client=client)
        if turn and turn.content.strip():
            turns.append(turn)
    return turns


def latest_assistant_turn(path: Path, client: str = "generic") -> AgentTurn | None:
    for turn in reversed(load_turns(path, client=client)):
        if turn.role == "assistant":
            return turn
    return None


def _claude_transcript_candidates(project_root: Path) -> list[Path]:
    local_runtime = project_root / ".devcouncil" / "live" / "claude"
    candidates = list(local_runtime.glob("*.jsonl"))
    if CLAUDE_TRANSCRIPT_ROOT.exists():
        candidates.extend(CLAUDE_TRANSCRIPT_ROOT.rglob("*.jsonl"))
    return sorted(set(candidates), key=lambda path: path.stat().st_mtime if path.exists() else 0, reverse=True)


def _safe_lines(path: Path) -> Iterable[str]:
    try:
        return path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def _read_jsonl(path: Path) -> Iterable[dict[str, Any]]:
    for line in _safe_lines(path):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            yield value


def _turn_from_record(raw: dict[str, Any], session_id: str, turn_index: int, client: str) -> AgentTurn | None:
    role = _role(raw)
    content = _content(raw)
    if not content:
        return None
    turn_id = str(raw.get("uuid") or raw.get("id") or raw.get("message_id") or f"turn-{turn_index}")
    return AgentTurn(
        session_id=str(raw.get("sessionId") or raw.get("session_id") or session_id),
        turn_id=turn_id,
        source=client,
        role=role,
        content=content,
        timestamp=raw.get("timestamp") or raw.get("created_at"),
        raw=raw,
    )


def _role(raw: dict[str, Any]) -> str:
    role = raw.get("role")
    if isinstance(role, str):
        return role if role in {"user", "assistant", "system", "tool"} else "unknown"
    message = raw.get("message")
    if isinstance(message, dict):
        nested = message.get("role")
        if isinstance(nested, str):
            return nested if nested in {"user", "assistant", "system", "tool"} else "unknown"
    record_type = raw.get("type")
    if record_type in {"user", "assistant", "system"}:
        return str(record_type)
    return "unknown"


def _content(raw: dict[str, Any]) -> str:
    direct = raw.get("content") or raw.get("text")
    if isinstance(direct, str):
        return direct
    message = raw.get("message")
    if isinstance(message, dict):
        nested = message.get("content")
        if isinstance(nested, str):
            return nested
        if isinstance(nested, list):
            return "\n".join(_content_block_text(block) for block in nested).strip()
    if isinstance(direct, list):
        return "\n".join(_content_block_text(block) for block in direct).strip()
    return ""


def _content_block_text(block: Any) -> str:
    if isinstance(block, str):
        return block
    if isinstance(block, dict):
        value = block.get("text") or block.get("content")
        return value if isinstance(value, str) else ""
    return ""
