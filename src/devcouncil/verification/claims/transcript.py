"""Extract the final assistant message from a Claude transcript JSONL."""

from __future__ import annotations

import json
import re
from pathlib import Path

_SENTENCE_SPLIT = re.compile(r"(?<=[.!?])\s+")


def _text_of(entry: dict) -> str | None:
    message = entry.get("message")
    if not isinstance(message, dict):
        return None
    content = message.get("content")
    if isinstance(content, str):
        return content.strip() or None
    if isinstance(content, list):
        texts = [
            block.get("text", "")
            for block in content
            if isinstance(block, dict) and block.get("type") == "text"
        ]
        joined = "\n".join(t for t in texts if t.strip())
        return joined.strip() or None
    return None


def last_assistant_text(path: Path) -> str | None:
    """Last assistant message that contains text (tool-use-only turns skipped)."""
    try:
        lines = path.read_text(encoding="utf-8-sig", errors="replace").splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(entry, dict) or entry.get("type") != "assistant":
            continue
        text = _text_of(entry)
        if text:
            return text
    return None


def last_assistant_sentence(path: Path) -> str | None:
    """Last sentence of the last assistant text, or None."""
    text = last_assistant_text(path)
    if not text:
        return None
    parts = [p.strip() for p in _SENTENCE_SPLIT.split(text) if p.strip()]
    return parts[-1] if parts else text.strip()


def ends_on_open_question(path: Path) -> bool:
    """Heuristic: last assistant turn ends with '?' and no later user turn."""
    try:
        lines = path.read_text(encoding="utf-8-sig", errors="replace").splitlines()
    except OSError:
        return False
    last_assistant_idx = -1
    last_user_idx = -1
    last_assistant_msg: str | None = None
    for i, line in enumerate(lines):
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(entry, dict):
            continue
        etype = entry.get("type")
        if etype == "assistant":
            text = _text_of(entry)
            if text:
                last_assistant_idx = i
                last_assistant_msg = text
        elif etype == "user":
            last_user_idx = i
    if last_assistant_idx < 0 or not last_assistant_msg:
        return False
    if last_user_idx > last_assistant_idx:
        return False
    return last_assistant_msg.rstrip().endswith("?")
