"""Shared Claude Code hook configuration constants."""

from __future__ import annotations

SESSION_START_MATCHER = "startup|resume|clear|compact"

CLAUDE_ASSIST_LIFECYCLE_EVENTS = (
    "Stop",
    "SessionStart",
    "UserPromptSubmit",
    "SessionEnd",
    "PreCompact",
    "PostCompact",
    "SubagentStop",
    "Notification",
)
