from __future__ import annotations

import json
import shutil
from pathlib import Path
from typing import Any, Iterable, Literal

from devcouncil.live.models import AgentSession, AgentTurn, session_id_from_path
from devcouncil.utils.json_persist import read_json

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


def claude_project_slug(project_root: Path | str) -> str:
    """Encode a path the way Claude Code names dirs under ``~/.claude/projects/``."""
    resolved = Path(project_root).expanduser().resolve()
    return resolved.as_posix().replace("/", "-")


def _read_claude_session_id(project_root: Path, task_id: str | None) -> str | None:
    if not task_id:
        return None
    path = project_root / ".devcouncil" / "sessions" / f"{task_id}-claude.json"
    if not path.is_file():
        return None
    try:
        data = read_json(path) or {}
    except (json.JSONDecodeError, OSError):
        return None
    session_id = str(data.get("session_id") or "").strip()
    return session_id or None


def claude_transcript_for_session(project_root: Path, session_id: str) -> Path | None:
    """Return the native Claude JSONL for ``session_id`` in this project, if present."""
    if not session_id:
        return None
    slug_dir = CLAUDE_TRANSCRIPT_ROOT / claude_project_slug(project_root)
    direct = slug_dir / f"{session_id}.jsonl"
    if direct.is_file():
        return direct
    for candidate in _claude_transcript_candidates(project_root):
        if candidate.stem == session_id:
            return candidate
    return None


def claude_transcript_for_task(project_root: Path, task_id: str) -> Path | None:
    """Resolve the Claude transcript DevCouncil pinned for ``task_id``."""
    session_id = _read_claude_session_id(project_root, task_id)
    if session_id:
        path = claude_transcript_for_session(project_root, session_id)
        if path is not None:
            return path
    return None


def mirror_claude_transcript(project_root: Path, session_id: str) -> Path | None:
    """Copy the native Claude JSONL into ``.devcouncil/live/claude/`` for discovery.

    Headless ``dev run``/``dev e2e`` pins a session id up front; mirroring the native
    transcript into the project makes live review resilient even when Claude's global
    project slug is slow to appear or ``discover_sessions`` would otherwise race."""
    source = claude_transcript_for_session(project_root, session_id)
    if source is None or not source.is_file():
        return None
    dest_dir = project_root / ".devcouncil" / "live" / "claude"
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / f"{session_id}.jsonl"
    try:
        shutil.copy2(source, dest)
    except OSError:
        return None
    return dest


def _claude_transcript_candidates(project_root: Path) -> list[Path]:
    local_runtime = project_root / ".devcouncil" / "live" / "claude"
    candidates = list(local_runtime.glob("*.jsonl"))
    if CLAUDE_TRANSCRIPT_ROOT.exists():
        slug_dir = CLAUDE_TRANSCRIPT_ROOT / claude_project_slug(project_root)
        if slug_dir.is_dir():
            candidates.extend(slug_dir.glob("*.jsonl"))
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
