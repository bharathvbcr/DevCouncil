"""Rank 16 — task dependency DAG: plan-gate validation and topological execution order."""

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import Task
from devcouncil.gating.policy import GatePolicy, topological_order


def _req():
    return Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
    )


def _task(tid, depends_on=None):
    return Task(
        id=tid, title=tid, description="d", requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"], depends_on=depends_on or [],
    )


def test_plan_gate_passes_valid_dependencies():
    tasks = [_task("TASK-001"), _task("TASK-002", depends_on=["TASK-001"])]
    result = GatePolicy().check_plan_approval([_req()], tasks)
    assert result.passed


def test_plan_gate_blocks_unknown_dependency():
    tasks = [_task("TASK-002", depends_on=["TASK-999"])]
    result = GatePolicy().check_plan_approval([_req()], tasks)
    assert not result.passed
    assert any("unknown task" in g.description for g in result.gaps if g.blocking)


def test_plan_gate_blocks_dependency_cycle():
    tasks = [_task("TASK-001", depends_on=["TASK-002"]), _task("TASK-002", depends_on=["TASK-001"])]
    result = GatePolicy().check_plan_approval([_req()], tasks)
    assert not result.passed
    assert any("cycle" in g.description.lower() for g in result.gaps if g.blocking)


def test_topological_order_respects_dependencies():
    # Declared out of order: C depends on B depends on A.
    tasks = [_task("C", depends_on=["B"]), _task("A"), _task("B", depends_on=["A"])]
    order = [t.id for t in topological_order(tasks)]
    assert order.index("A") < order.index("B") < order.index("C")


def test_topological_order_stable_for_independent_tasks():
    tasks = [_task("A"), _task("B"), _task("C")]
    assert [t.id for t in topological_order(tasks)] == ["A", "B", "C"]


def test_topological_order_falls_back_on_cycle():
    tasks = [_task("A", depends_on=["B"]), _task("B", depends_on=["A"])]
    # A cycle has no valid order; fall back to the original order rather than dropping tasks.
    assert {t.id for t in topological_order(tasks)} == {"A", "B"}
    assert len(topological_order(tasks)) == 2
