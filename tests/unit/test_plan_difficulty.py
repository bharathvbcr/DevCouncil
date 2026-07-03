"""Tests for plan-time difficulty tagging."""

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.planning.plan_difficulty import (
    annotate_tasks_with_difficulty,
    overscoped_hard_warnings,
)
from devcouncil.verification.difficulty import difficulty_score


def _big_task() -> Task:
    return Task(
        id="TASK-BIG",
        title="Migrate everything",
        description="Large refactor across modules",
        acceptance_criterion_ids=[f"AC-{i}" for i in range(6)],
        planned_files=[
            PlannedFile(path=f"src/m{i}.py", reason="r", allowed_change="modify")
            for i in range(5)
        ],
        depends_on=["T1", "T2"],
    )


def test_annotate_sets_difficulty_when_missing():
    task = _big_task()
    assert task.difficulty is None
    annotated = annotate_tasks_with_difficulty([task])
    assert annotated[0].difficulty == "hard"


def test_overscoped_hard_warning_emitted():
    task = _big_task()
    assert difficulty_score(task) >= 4
    warnings = overscoped_hard_warnings([task])
    assert any("TASK-BIG" in w for w in warnings)
