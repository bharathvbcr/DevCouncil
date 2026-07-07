from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path

from devcouncil.live.models import AgentTurn, CardStatus, CritiqueCard, Verdict
from devcouncil.utils.json_persist import read_model_json, write_model_json

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

# Word-boundary matchers so "done" matches "I'm done" but not "abandoned"/"undone".
_COMPLETION_RE = re.compile(
    r"\b(done|complete|completed|finished|implemented|fixed|ready|all set|"
    r"ship it|good to go|works now|it works)\b"
)
# An agent asserting its verification actually passed (the claim we cross-check).
_PASS_CLAIM_RE = re.compile(
    r"(tests?\s+(?:are\s+|now\s+)?pass(?:ing|ed|es)?"
    r"|all\s+(?:tests?|checks?|cases?)\s+pass"
    r"|passing\s+tests?"
    r"|\bverified\b|verification\s+(?:pass|succeed)"
    r"|tests?\s+green|green\s+tests?"
    r"|(?:ran|run)\s+[^.\n]{0,40}?\bpass)"
)
# Negations that flip a nearby claim ("not done", "tests do not pass", "still failing").
_NEGATION_RE = re.compile(
    r"\b(not|isn'?t|aren'?t|won'?t|can'?t|cannot|haven'?t|hasn'?t|don'?t|"
    r"doesn'?t|didn'?t|no longer|never|yet to|still need|still failing|"
    r"not yet|unable|fail(?:s|ing|ed)?)\b"
)
_NEGATION_WINDOW = 30


def _claim_present(pattern: re.Pattern[str], lower: str) -> bool:
    """True if `pattern` matches and is not negated by a word shortly before it."""
    for match in pattern.finditer(lower):
        prefix = lower[max(0, match.start() - _NEGATION_WINDOW):match.start()]
        if _NEGATION_RE.search(prefix):
            continue
        return True
    return False


@dataclass
class _TaskGrounding:
    """A snapshot of a task's real verification state from the artifact graph."""

    task_id: str
    status: str
    blocking_gaps: int
    failing_commands: int
    acs_total: int
    acs_passing: int

    @property
    def acs_unproven(self) -> int:
        return max(0, self.acs_total - self.acs_passing)

    @property
    def is_satisfied(self) -> bool:
        return (
            self.status in ("verified", "done")
            and self.blocking_gaps == 0
            and (self.acs_total == 0 or self.acs_passing >= self.acs_total)
        )


def _load_task_grounding(project_root: Path, task_id: str | None) -> _TaskGrounding | None:
    """Load the scoped task's real verification state so claims can be checked
    against evidence instead of trusted on the agent's word. Best-effort: any
    failure (no DB, unknown task) returns None and the caller falls back to the
    pure-heuristic review."""
    if not task_id:
        return None
    try:
        from devcouncil.storage.db import get_db
        from devcouncil.storage.repositories import ArtifactGraphRepository

        db = get_db(project_root)
        if not db:
            return None
        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
    except Exception:
        return None

    task = graph.tasks.get(task_id)
    if task is None:
        return None

    blocking = [g for g in graph.gaps.values() if g.task_id == task_id and g.blocking]
    failing = [g for g in blocking if g.gap_type == "test_failed"]
    ac_ids = set(task.acceptance_criterion_ids)
    passing_ac = {
        ev.acceptance_criterion_id
        for ev in graph.test_evidence
        if ev.acceptance_criterion_id in ac_ids and getattr(ev, "status", "") == "passed"
    }
    return _TaskGrounding(
        task_id=task_id,
        status=task.status,
        blocking_gaps=len(blocking),
        failing_commands=len(failing),
        acs_total=len(ac_ids),
        acs_passing=len(passing_ac),
    )


def review_turn(
    turn: AgentTurn,
    project_root: Path,
    client: str | None = None,
    task_id: str | None = None,
) -> CritiqueCard:
    """Generate a deterministic critique card for an agent response.

    When ``task_id`` resolves to a known task, completion/verification claims are
    checked against the task's real artifact state (status, blocking gaps, passing
    acceptance-criterion evidence) instead of being trusted by keyword alone. With
    no task state available it falls back to the lightweight keyword heuristic.
    """
    content = turn.content.strip()
    lower = content.lower()
    concerns: list[str] = []
    alternatives: list[str] = []
    evidence_requests: list[str] = []
    verdict: Verdict = "Approved"

    grounding = _load_task_grounding(project_root, task_id)

    risky_terms = [term for term in RISK_TERMS if term in lower]
    if risky_terms:
        concerns.append(f"Response contains risky implementation language: {', '.join(risky_terms[:4])}.")
        alternatives.append("Replace risky shortcuts with a scoped implementation and explicit rollback or verification path.")

    claims_completion = _claim_present(_COMPLETION_RE, lower)
    claims_passing = _claim_present(_PASS_CLAIM_RE, lower)

    if grounding is not None:
        # Evidence-grounded review: cross-check the agent's claims against reality.
        if claims_passing and grounding.failing_commands > 0:
            concerns.append(
                f"Agent claims verification passes, but DevCouncil recorded "
                f"{grounding.failing_commands} failing verification command(s) for "
                f"task {grounding.task_id}."
            )
            evidence_requests.append(
                f"Re-run 'dev verify {grounding.task_id}' and fix the failing command(s) "
                "before claiming success."
            )
            verdict = "Critical Issues"
        elif (claims_completion or claims_passing) and not grounding.is_satisfied:
            details = [f"task {grounding.task_id} is '{grounding.status}'"]
            if grounding.blocking_gaps:
                details.append(f"{grounding.blocking_gaps} blocking gap(s)")
            if grounding.acs_unproven:
                details.append(
                    f"{grounding.acs_unproven}/{grounding.acs_total} acceptance "
                    "criteria still lack passing evidence"
                )
            concerns.append(
                "Completion claim is not yet backed by DevCouncil evidence: "
                + ", ".join(details) + "."
            )
            evidence_requests.append(
                f"Run 'dev verify {grounding.task_id}' and resolve the gaps so the "
                "claim is supported by passing evidence."
            )
        elif claims_completion and grounding.is_satisfied:
            alternatives.append(
                f"Completion is corroborated by passing evidence for task {grounding.task_id}; "
                "proceed."
            )
    elif claims_completion and not any(term in lower for term in EVIDENCE_TERMS):
        # No task state to ground against: best-effort keyword heuristic.
        concerns.append("The response appears to claim completion without naming verification evidence.")
        evidence_requests.append("State the exact commands, checks, or reviewed artifacts that prove the change.")

    if _mentions_broad_change(lower):
        concerns.append("The response suggests broad codebase changes; confirm they are authorized by the active DevCouncil task.")
        alternatives.append("Split broad work into smaller planned files and run DevCouncil gates before marking it done.")

    if "todo" in lower or "follow-up" in lower or "later" in lower:
        evidence_requests.append("List any remaining TODOs as DevCouncil gaps or repair tasks instead of burying them in chat.")

    if concerns and verdict == "Approved":
        verdict = "Concerns"
    if any(term in lower for term in ("--no-verify", "reset --hard", "force push", "ignore failing")):
        verdict = "Critical Issues"

    if not alternatives and verdict == "Approved":
        alternatives.append("Proceed, but keep the final answer tied to changed files and verification evidence.")

    summary = "No blocking critique found." if verdict == "Approved" else concerns[0]
    message_for_agent = _message_for_agent(verdict, concerns, evidence_requests)
    card_id = _card_id(turn)
    return CritiqueCard(
        schema="devcouncil.critique_card.v1",
        id=card_id,
        session_id=turn.session_id,
        turn_id=turn.turn_id,
        task_id=task_id,
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
    write_model_json(path, card)
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
            cards.append(read_model_json(path, CritiqueCard))
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


def load_card_by_id(project_root: Path, card_id: str) -> CritiqueCard | None:
    """Read a single card directly by id, avoiding a scan of every card file."""
    path = card_path(project_root, card_id)
    if not path.exists():
        return None
    try:
        return read_model_json(path, CritiqueCard)
    except Exception:
        return None


def get_card(project_root: Path, card_id: str) -> CritiqueCard | None:
    return load_card_by_id(project_root, card_id)


def update_card_status(project_root: Path, card_id: str, status: CardStatus) -> CritiqueCard | None:
    cards_dir = project_root / ".devcouncil" / "live" / "cards"
    path = cards_dir / f"{card_id}.json"
    if not path.exists():
        return None
    card = read_model_json(path, CritiqueCard)
    updated = card.model_copy(update={"status": status})
    write_model_json(path, updated)
    return updated


def unresolved_blocking_cards(
    project_root: Path,
    task_id: str | None = None,
    *,
    cards: list[CritiqueCard] | None = None,
) -> list[CritiqueCard]:
    source = cards if cards is not None else load_cards(project_root)
    return [
        card for card in source
        if card.status == "open" and card.verdict == "Critical Issues" and card.blocks_gate
        and (task_id is None or card.task_id in {None, task_id})
    ]


def _card_id(turn: AgentTurn) -> str:
    digest = hashlib.sha256(f"{turn.session_id}:{turn.turn_id}:{turn.content}".encode("utf-8")).hexdigest()
    return f"CARD-{digest[:12]}"


def _mentions_broad_change(lower: str) -> bool:
    return any(phrase in lower for phrase in (
        "refactor the entire",
        "rewrite",
        "all files",
        "every file",
        "across the codebase",
    ))


def _message_for_agent(verdict: Verdict, concerns: list[str], evidence_requests: list[str]) -> str:
    if verdict == "Approved":
        return "Continue, but keep the next response grounded in changed files and verification evidence."
    pieces = ["Pause and address this review before proceeding."]
    pieces.extend(concerns)
    pieces.extend(evidence_requests)
    return " ".join(pieces)
