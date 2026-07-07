"""RequirementCoverageCheck — pure plan-level check that every requirement is
mapped to at least one task."""

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.gating.checks.requirement_coverage import RequirementCoverageCheck


def _requirement(req_id: str, title: str) -> Requirement:
    return Requirement(
        id=req_id,
        title=title,
        description=f"{title} requirement",
        priority="high",
        source="planner",
    )


def _task(task_id: str, requirement_ids: list[str]) -> Task:
    return Task(
        id=task_id,
        title=f"Implement {task_id}",
        description="implementation task",
        requirement_ids=requirement_ids,
    )


def test_all_requirements_covered_produces_no_gaps():
    requirements = [_requirement("REQ-001", "Auth"), _requirement("REQ-002", "Logout")]
    tasks = [
        _task("TASK-001", ["REQ-001"]),
        # Coverage is a union across tasks; overlapping links are fine.
        _task("TASK-002", ["REQ-002", "REQ-001"]),
    ]

    assert RequirementCoverageCheck().check(requirements, tasks) == []


def test_unmapped_requirement_produces_blocking_gap():
    requirements = [_requirement("REQ-001", "Auth"), _requirement("REQ-002", "Password reset")]
    tasks = [_task("TASK-001", ["REQ-001"])]

    gaps = RequirementCoverageCheck().check(requirements, tasks)

    assert len(gaps) == 1
    gap = gaps[0]
    assert gap.id == "GAP-PLAN-REQ-002-UNMAPPED"
    assert gap.gap_type == "requirement_not_planned"
    assert gap.requirement_id == "REQ-002"
    assert gap.severity == "high"
    assert gap.blocking is True
    assert "Password reset" in gap.description


def test_no_tasks_flags_every_requirement():
    requirements = [_requirement("REQ-001", "Auth"), _requirement("REQ-002", "Logout")]

    gaps = RequirementCoverageCheck().check(requirements, [])

    assert [g.id for g in gaps] == [
        "GAP-PLAN-REQ-001-UNMAPPED",
        "GAP-PLAN-REQ-002-UNMAPPED",
    ]
    assert all(g.blocking for g in gaps)
