from types import SimpleNamespace

from devcouncil.domain.assumption import Assumption
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.gating.policy import GatePolicy


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Auth",
        description="Auth requirement",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-001",
                description="Auth behavior is tested",
                verification_method="unit_test",
            )
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement auth",
        description="Implement auth",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path="src/auth.py", reason="auth logic", allowed_change="modify"),
        ],
    )


def test_plan_gate_passes_complete_plan():
    result = GatePolicy().check_plan_approval([_requirement()], [_task()])
    assert result.passed


def test_plan_gate_blocks_open_high_impact_assumption():
    assumption = Assumption(
        id="ASM-001",
        statement="Provider has single-use token support",
        confidence="medium",
        impact="high",
        reversible=True,
        requires_user_confirmation=True,
        status="open",
    )

    result = GatePolicy().check_plan_approval([_requirement()], [_task()], assumptions=[assumption])

    assert not result.passed
    assert any(gap.gap_type == "assumption_violated" for gap in result.gaps)


def test_plan_gate_blocks_open_high_critique_finding():
    finding = CritiqueFinding(
        id="FIND-001",
        source_agent="critic_a",
        target_plan_id="PLAN-B",
        severity="high",
        finding_type="security_risk",
        claim="Plan does not hash reset tokens",
        falsifiable_check="Inspect token persistence",
        status="open",
    )

    result = GatePolicy().check_plan_approval([_requirement()], [_task()], findings=[finding])

    assert not result.passed
    assert any("FIND-001" in gap.id for gap in result.gaps)


def test_plan_gate_blocks_unanswered_question():
    question = SimpleNamespace(id="Q-001", question="Should reset invalidate sessions?")

    result = GatePolicy().check_plan_approval(
        [_requirement()],
        [_task()],
        blocking_questions=[question],
    )

    assert not result.passed
    assert any("Q-001" in gap.id for gap in result.gaps)


def test_plan_gate_blocks_unknown_task_links():
    task = _task()
    task.requirement_ids = ["REQ-MISSING"]
    task.acceptance_criterion_ids = ["AC-MISSING"]

    result = GatePolicy().check_plan_approval([_requirement()], [task])

    assert not result.passed
    assert any("UNKNOWN-REQ" in gap.id for gap in result.gaps)
    assert any("UNKNOWN-AC" in gap.id for gap in result.gaps)
