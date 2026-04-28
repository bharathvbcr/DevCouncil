from __future__ import annotations

from pathlib import Path

from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.live.models import CritiqueCard
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import RequirementRepository, TaskRepository


def build_live_repair_prompt(project_root: Path, card: CritiqueCard) -> str:
    """Build a ready-to-paste repair prompt for a live-review critique card."""
    prompt = [
        f"# Repair Live Review Card {card.id}",
        "",
        "A DevCouncil live-review card is blocking or warning on the current coding-agent session.",
        "Address the critique directly, then provide exact verification evidence.",
        "",
        "## Card",
        f"- Verdict: {card.verdict}",
        f"- Status: {card.status}",
        f"- Task: {card.task_id or '(unscoped)'}",
        f"- Summary: {card.summary}",
    ]
    if card.concerns:
        prompt.extend(["", "## Concerns"])
        prompt.extend(f"- {item}" for item in card.concerns)
    if card.alternatives:
        prompt.extend(["", "## Safer Alternatives"])
        prompt.extend(f"- {item}" for item in card.alternatives)
    if card.evidence_requests:
        prompt.extend(["", "## Required Evidence"])
        prompt.extend(f"- {item}" for item in card.evidence_requests)
    if card.message_for_agent:
        prompt.extend(["", "## Message For Agent", card.message_for_agent])

    task_prompt = _task_prompt(project_root, card.task_id)
    if task_prompt:
        prompt.extend(["", "## Original DevCouncil Task Contract", task_prompt])

    prompt.extend([
        "",
        "## Repair Instructions",
        "1. Do not bypass tests, hooks, or verification gates.",
        "2. Keep changes within the DevCouncil task contract when one is present.",
        "3. Address each concern above explicitly.",
        "4. Run the expected verification commands and report exact results.",
        f"5. After the repair is complete, ask the developer to run `dev watch resolve {card.id} --status resolved`.",
    ])
    return "\n".join(prompt).rstrip() + "\n"


def build_bulk_live_repair_prompt(project_root: Path, cards: list[CritiqueCard]) -> str:
    """Build a combined repair prompt for multiple live-review critique cards."""
    if not cards:
        return "# Live Review Repair\n\nNo blocking live-review cards found for this scope.\n"
    sections = [
        "# Repair Blocking Live Review Cards",
        "",
        f"DevCouncil found {len(cards)} blocking live-review card(s). Address each card below.",
    ]
    for index, card in enumerate(cards, start=1):
        sections.extend([
            "",
            f"---\n\n## Card {index}: {card.id}",
            "",
            build_live_repair_prompt(project_root, card).strip(),
        ])
    return "\n".join(sections).rstrip() + "\n"


def _task_prompt(project_root: Path, task_id: str | None) -> str | None:
    if not task_id:
        return None
    db = get_db(project_root)
    if not db:
        return None
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
        if not task:
            return None
        requirements = RequirementRepository(session).get_all()
    return PromptBuilder(project_root).build_task_prompt(task, requirements)
