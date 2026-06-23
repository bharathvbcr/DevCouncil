from types import SimpleNamespace
from pathlib import Path

from devcouncil.domain.assumption import Assumption
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.gating.policy import GatePolicy
from devcouncil.cli.commands.plan import _reconcile_findings


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


def test_reconciled_arbiter_findings_do_not_remain_open():
    accepted = CritiqueFinding(
        id="FIND-001",
        source_agent="critic_a",
        target_plan_id="PLAN-B",
        severity="high",
        finding_type="security_risk",
        claim="Plan does not hash reset tokens",
        falsifiable_check="Inspect token persistence",
        status="open",
    )
    rejected = accepted.model_copy(update={"id": "FIND-002"})
    decision = SimpleNamespace(
        accepted_finding_ids=["FIND-001"],
        rejected_finding_ids=[{"id": "FIND-002", "reason": "Covered elsewhere"}],
    )

    reconciled = _reconcile_findings([accepted, rejected], decision)

    assert [finding.status for finding in reconciled] == ["converted", "rejected"]
    result = GatePolicy().check_plan_approval([_requirement()], [_task()], findings=reconciled)
    assert result.passed


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


def test_task_ready_warns_but_allows_missing_commands_and_expected_evidence(monkeypatch):
    policy = GatePolicy()
    monkeypatch.setattr(policy.clean_git, "check", lambda project_root, task_id: [])

    result = policy.check_task_ready(_task(), Path("."))

    # A missing verification contract is a verify-time concern: readiness must not
    # block execution on it (the executor still needs to implement the code).
    assert result.passed
    contract_gaps = [
        gap for gap in result.gaps
        if "NO-COMMANDS" in gap.id or "NO-EXPECTED-EVIDENCE" in gap.id
    ]
    assert contract_gaps
    assert not any(gap.blocking for gap in contract_gaps)


def test_task_ready_passes_with_commands_and_expected_evidence(monkeypatch):
    policy = GatePolicy()
    monkeypatch.setattr(policy.clean_git, "check", lambda project_root, task_id: [])
    task = _task()
    task.allowed_commands = ["pytest tests/test_auth.py"]
    task.expected_tests = ["pytest tests/test_auth.py"]

    result = policy.check_task_ready(task, Path("."))

    assert result.passed
