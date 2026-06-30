from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Iterable, Literal

from devcouncil.live.models import AgentSession, AgentTurn, session_id_from_path

RoleName = Literal["user", "assistant", "system", "tool", "unknown"]
KNOWN_ROLES: set[RoleName] = {"user", "assistant", "system", "tool"}


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
            # len() over the already-materialized splitlines list avoids a
            # second Python-level pass and matches the previous line count.
            turns=len(_safe_lines(path)),
        ))
    def _updated_key(item: AgentSession) -> float:
        # updated_at is a stringified mtime; sort numerically so timestamps with
        # different digit counts (string sort would misorder them) compare correctly.
        try:
            return float(item.updated_at or 0.0)
        except (TypeError, ValueError):
            return 0.0

    return sorted(sessions, key=_updated_key, reverse=True)


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


# mtime-keyed cache: reloading and reversing every turn just to find the last
# assistant message is wasteful when the transcript hasn't changed.
_LATEST_ASSISTANT_CACHE: dict[str, tuple[float, AgentTurn | None]] = {}


def _scan_latest_assistant_turn(path: Path, client: str) -> AgentTurn | None:
    for turn in reversed(load_turns(path, client=client)):
        if turn.role == "assistant":
            return turn
    return None


def latest_assistant_turn(path: Path, client: str = "generic") -> AgentTurn | None:
    try:
        mtime = path.stat().st_mtime
    except OSError:
        return _scan_latest_assistant_turn(path, client)
    key = f"{path}\x00{client}"
    cached = _LATEST_ASSISTANT_CACHE.get(key)
    if cached is not None and cached[0] == mtime:
        return cached[1]
    result = _scan_latest_assistant_turn(path, client)
    _LATEST_ASSISTANT_CACHE[key] = (mtime, result)
    return result


def _claude_transcript_candidates(project_root: Path) -> list[Path]:
    local_runtime = project_root / ".devcouncil" / "live" / "claude"
    candidates = list(local_runtime.glob("*.jsonl"))
    if CLAUDE_TRANSCRIPT_ROOT.exists():
        candidates.extend(CLAUDE_TRANSCRIPT_ROOT.rglob("*.jsonl"))
    # Dedup, then sort by path for a deterministic, stat-free order. discover_sessions
    # re-sorts the resulting sessions by mtime; because that sort is stable, giving it a
    # deterministic input keeps the tie-break (equal-mtime sessions) stable across runs —
    # whereas an unordered set would let ties reorder run to run.
    return sorted(set(candidates))


def _safe_lines(path: Path) -> list[str]:
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


def _role(raw: dict[str, Any]) -> RoleName:
    role = raw.get("role")
    if isinstance(role, str):
        return role if role in KNOWN_ROLES else "unknown"
    message = raw.get("message")
    if isinstance(message, dict):
        nested = message.get("role")
        if isinstance(nested, str):
            return nested if nested in KNOWN_ROLES else "unknown"
    record_type = raw.get("type")
    if record_type in KNOWN_ROLES:
        return record_type
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
