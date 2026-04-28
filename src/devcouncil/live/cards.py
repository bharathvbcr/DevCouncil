from __future__ import annotations

import hashlib
import json
from pathlib import Path

from devcouncil.live.models import AgentTurn, CardStatus, CritiqueCard

RISK_TERMS = (
    "skip tests",
    "no tests",
    "untested",
    "ignore failing",
    "disable",
    "workaround",
    "quick hack",
    "hardcode",
    "force push",
    "--no-verify",
    "reset --hard",
)

EVIDENCE_TERMS = (
    "test",
    "pytest",
    "vitest",
    "npm test",
    "go test",
    "cargo test",
    "verification",
    "verified",
)


def review_turn(turn: AgentTurn, project_root: Path, client: str | None = None) -> CritiqueCard:
    """Generate a deterministic critique card for an agent response."""
    content = turn.content.strip()
    lower = content.lower()
    concerns: list[str] = []
    alternatives: list[str] = []
    evidence_requests: list[str] = []

    risky_terms = [term for term in RISK_TERMS if term in lower]
    if risky_terms:
        concerns.append(f"Response contains risky implementation language: {', '.join(risky_terms[:4])}.")
        alternatives.append("Replace risky shortcuts with a scoped implementation and explicit rollback or verification path.")

    if _looks_like_completion_claim(lower) and not any(term in lower for term in EVIDENCE_TERMS):
        concerns.append("The response appears to claim completion without naming verification evidence.")
        evidence_requests.append("State the exact commands, checks, or reviewed artifacts that prove the change.")

    if _mentions_broad_change(lower):
        concerns.append("The response suggests broad codebase changes; confirm they are authorized by the active DevCouncil task.")
        alternatives.append("Split broad work into smaller planned files and run DevCouncil gates before marking it done.")

    if "todo" in lower or "follow-up" in lower or "later" in lower:
        evidence_requests.append("List any remaining TODOs as DevCouncil gaps or repair tasks instead of burying them in chat.")

    verdict = "Approved"
    if concerns:
        verdict = "Concerns"
    if any(term in lower for term in ("--no-verify", "reset --hard", "force push", "ignore failing")):
        verdict = "Critical Issues"

    if not alternatives and verdict == "Approved":
        alternatives.append("Proceed, but keep the final answer tied to changed files and verification evidence.")

    summary = "No blocking critique found." if verdict == "Approved" else concerns[0]
    message_for_agent = _message_for_agent(verdict, concerns, evidence_requests)
    card_id = _card_id(turn)
    return CritiqueCard(
        id=card_id,
        session_id=turn.session_id,
        turn_id=turn.turn_id,
        client=client or turn.source,
        verdict=verdict,
        summary=summary,
        concerns=concerns,
        alternatives=alternatives,
        evidence_requests=evidence_requests,
        message_for_agent=message_for_agent,
    )


def save_card(project_root: Path, card: CritiqueCard) -> Path:
    cards_dir = project_root / ".devcouncil" / "live" / "cards"
    cards_dir.mkdir(parents=True, exist_ok=True)
    path = cards_dir / f"{card.id}.json"
    path.write_text(card.model_dump_json(indent=2) + "\n", encoding="utf-8")
    return path


def card_path(project_root: Path, card_id: str) -> Path:
    return project_root / ".devcouncil" / "live" / "cards" / f"{card_id}.json"


def load_cards(project_root: Path) -> list[CritiqueCard]:
    cards_dir = project_root / ".devcouncil" / "live" / "cards"
    if not cards_dir.exists():
        return []
    cards: list[CritiqueCard] = []
    for path in sorted(cards_dir.glob("*.json"), key=lambda item: item.stat().st_mtime, reverse=True):
        try:
            cards.append(CritiqueCard.model_validate(json.loads(path.read_text(encoding="utf-8"))))
        except Exception:
            continue
    return cards


def filter_cards(
    cards: list[CritiqueCard],
    *,
    task_id: str | None = None,
    status: str | None = None,
    verdict: str | None = None,
    client: str | None = None,
) -> tuple[list[CritiqueCard], str | None, str | None]:
    normalized_status = status.lower() if status else None
    if normalized_status and normalized_status not in {"open", "resolved", "ignored"}:
        return [], "--status must be open, resolved, or ignored.", "status"

    verdict_map = {
        "approved": "Approved",
        "concerns": "Concerns",
        "critical": "Critical Issues",
        "critical issues": "Critical Issues",
    }
    normalized_verdict = None
    if verdict:
        normalized_verdict = verdict_map.get(verdict.lower())
        if not normalized_verdict:
            return [], "--verdict must be approved, concerns, or critical.", "verdict"

    normalized_client = client.lower() if client else None
    filtered = []
    for card in cards:
        if task_id and card.task_id != task_id:
            continue
        if normalized_status and card.status != normalized_status:
            continue
        if normalized_verdict and card.verdict != normalized_verdict:
            continue
        if normalized_client and card.client.lower() != normalized_client:
            continue
        filtered.append(card)
    return filtered, None, None


def get_card(project_root: Path, card_id: str) -> CritiqueCard | None:
    for card in load_cards(project_root):
        if card.id == card_id:
            return card
    return None


def update_card_status(project_root: Path, card_id: str, status: CardStatus) -> CritiqueCard | None:
    cards_dir = project_root / ".devcouncil" / "live" / "cards"
    path = cards_dir / f"{card_id}.json"
    if not path.exists():
        return None
    card = CritiqueCard.model_validate(json.loads(path.read_text(encoding="utf-8")))
    updated = card.model_copy(update={"status": status})
    path.write_text(updated.model_dump_json(indent=2) + "\n", encoding="utf-8")
    return updated


def unresolved_blocking_cards(project_root: Path, task_id: str | None = None) -> list[CritiqueCard]:
    return [
        card for card in load_cards(project_root)
        if card.status == "open" and card.verdict == "Critical Issues"
        and (task_id is None or card.task_id in {None, task_id})
    ]


def _card_id(turn: AgentTurn) -> str:
    digest = hashlib.sha1(f"{turn.session_id}:{turn.turn_id}:{turn.content}".encode("utf-8")).hexdigest()
    return f"CARD-{digest[:12]}"


def _looks_like_completion_claim(lower: str) -> bool:
    return any(phrase in lower for phrase in (
        "done",
        "completed",
        "implemented",
        "fixed",
        "ready",
        "all set",
    ))


def _mentions_broad_change(lower: str) -> bool:
    return any(phrase in lower for phrase in (
        "refactor the entire",
        "rewrite",
        "all files",
        "every file",
        "across the codebase",
    ))


def _message_for_agent(verdict: str, concerns: list[str], evidence_requests: list[str]) -> str:
    if verdict == "Approved":
        return "Continue, but keep the next response grounded in changed files and verification evidence."
    pieces = ["Pause and address this review before proceeding."]
    pieces.extend(concerns)
    pieces.extend(evidence_requests)
    return " ".join(pieces)
