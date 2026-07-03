"""Convert blocking spec questions into non-blocking assumptions."""

from __future__ import annotations

from devcouncil.domain.assumption import Assumption
from devcouncil.planning.spec_service import BlockingQuestion


def assumption_id_from_question(question_id: str) -> str:
    """Derive a stable assumption id from a blocking-question id."""
    if question_id.startswith("ASM-"):
        return question_id
    if question_id.startswith("Q-"):
        return f"ASM-from-{question_id}"
    return f"ASM-from-Q-{question_id}"


def blocking_question_to_assumption(question: BlockingQuestion) -> Assumption:
    """Map one blocking question to a low-confidence assumption that does not block approval."""
    statement = question.question
    if question.reason:
        statement = f"{question.question} (Context: {question.reason})"
    return Assumption(
        id=assumption_id_from_question(question.id),
        statement=statement,
        confidence="low",
        impact="medium",
        reversible=True,
        requires_user_confirmation=False,
        status="open",
    )


def convert_blocking_questions_to_assumptions(
    assumptions: list[Assumption],
    blocking_questions: list[BlockingQuestion],
) -> tuple[list[Assumption], list[BlockingQuestion]]:
    """Append converted questions as assumptions and return an empty blocking list."""
    if not blocking_questions:
        return assumptions, blocking_questions

    existing_ids = {assumption.id for assumption in assumptions}
    converted: list[Assumption] = []
    for question in blocking_questions:
        assumption = blocking_question_to_assumption(question)
        candidate_id = assumption.id
        suffix = 1
        while candidate_id in existing_ids:
            candidate_id = f"{assumption.id}-{suffix}"
            suffix += 1
        if candidate_id != assumption.id:
            assumption = assumption.model_copy(update={"id": candidate_id})
        existing_ids.add(assumption.id)
        converted.append(assumption)

    return [*assumptions, *converted], []
