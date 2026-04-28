from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field


Verdict = Literal["Approved", "Concerns", "Critical Issues"]
CardStatus = Literal["open", "resolved", "ignored"]


class AgentTurn(BaseModel):
    """A normalized coding-agent conversation turn."""

    session_id: str
    turn_id: str
    source: str = "generic"
    role: Literal["user", "assistant", "system", "tool", "unknown"] = "unknown"
    content: str = ""
    timestamp: str | None = None
    raw: dict[str, Any] = Field(default_factory=dict)


class AgentSession(BaseModel):
    """A discovered coding-agent transcript."""

    id: str
    client: str
    transcript_path: str
    updated_at: str | None = None
    turns: int = 0


class CritiqueCard(BaseModel):
    """Sage-style response review, shaped for DevCouncil gates."""

    model_config = ConfigDict(populate_by_name=True, serialize_by_alias=True)

    schema_version: str = Field("devcouncil.critique_card.v1", alias="schema")
    id: str
    session_id: str
    turn_id: str
    task_id: str | None = None
    client: str
    verdict: Verdict
    summary: str
    concerns: list[str] = Field(default_factory=list)
    alternatives: list[str] = Field(default_factory=list)
    message_for_agent: str = ""
    evidence_requests: list[str] = Field(default_factory=list)
    status: CardStatus = "open"
    created_at: str = Field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
    source_path: str | None = None

    @property
    def blocks_completion(self) -> bool:
        return self.verdict == "Critical Issues"


def session_id_from_path(path: Path) -> str:
    return path.stem.replace(".", "-")
