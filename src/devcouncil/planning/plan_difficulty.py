"""Plan-time difficulty tagging so the council sees rigor before execution.

The verifier's deterministic estimator runs here on every task the arbiter emits
without an explicit ``Task.difficulty``. Overscoped tasks that score ``hard`` get
a human-visible warning so planners can split them before agents start work.
"""

from __future__ import annotations

import logging
from typing import List, Tuple

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.verification.difficulty import (
    _HARD_THRESHOLD,
    difficulty_score,
    estimate_difficulty,
)

logger = logging.getLogger(__name__)


def annotate_tasks_with_difficulty(
    tasks: List[Task],
    requirements: List[Requirement] | None = None,
) -> List[Task]:
    """Set ``Task.difficulty`` from the estimator when the arbiter left it unset."""
    annotated: List[Task] = []
    for task in tasks:
        if task.difficulty in ("easy", "normal", "hard"):
            annotated.append(task)
            continue
        difficulty = estimate_difficulty(task, requirements)
        annotated.append(task.model_copy(update={"difficulty": difficulty}))
    return annotated


def overscoped_hard_warnings(
    tasks: List[Task],
    requirements: List[Requirement] | None = None,
) -> List[str]:
    """Advisory messages for tasks that score hard and may need splitting."""
    warnings: List[str] = []
    for task in tasks:
        score = difficulty_score(task, requirements)
        if score < _HARD_THRESHOLD:
            continue
        writable = [pf for pf in task.planned_files if pf.allowed_change != "read_only"]
        if len(writable) >= 4 or len(task.acceptance_criterion_ids) >= 5:
            warnings.append(
                f"{task.id} ({task.title!r}) scores hard ({score}): "
                f"{len(writable)} writable file(s), "
                f"{len(task.acceptance_criterion_ids)} acceptance criterion(s). "
                "Consider splitting into smaller tasks before execution."
            )
    return warnings


def apply_plan_difficulty(
    tasks: List[Task],
    requirements: List[Requirement] | None = None,
) -> Tuple[List[Task], List[str]]:
    """Annotate difficulty and return any overscoped-hard warnings."""
    annotated = annotate_tasks_with_difficulty(tasks, requirements)
    warnings = overscoped_hard_warnings(annotated, requirements)
    for msg in warnings:
        logger.info("Plan difficulty: %s", msg)
    return annotated, warnings
