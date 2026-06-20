"""Cross-task acceptance reconciliation.

When the planner splits "implement X" and "add tests for X" into separate tasks that
share acceptance criteria, the implement task must not stay blocked for criteria the
test task already proved. A criterion proven by passing evidence in ANY task is proven
for every task that shares it.
"""

from devcouncil.cli.commands.verify import reconcile_cross_task_acceptance
from devcouncil.domain.gap import Gap


def _ac_gap(gap_id: str, ac_id: str, *, blocking: bool = True) -> Gap:
    return Gap(
        id=gap_id,
        severity="high",
        gap_type="acceptance_criteria_unproven",
        task_id="implement_x",
        description=f"{ac_id} unproven",
        recommended_fix="prove it",
        blocking=blocking,
        acceptance_criterion_id=ac_id,
    )


def test_clears_acceptance_gap_proven_elsewhere():
    gaps = [_ac_gap("G1", "AC-1"), _ac_gap("G2", "AC-2")]
    kept = reconcile_cross_task_acceptance(gaps, proven_acs={"AC-1", "AC-2"})
    assert kept == []


def test_keeps_acceptance_gap_not_proven_anywhere():
    gaps = [_ac_gap("G1", "AC-1"), _ac_gap("G2", "AC-2")]
    kept = reconcile_cross_task_acceptance(gaps, proven_acs={"AC-1"})
    assert [g.id for g in kept] == ["G2"]


def test_does_not_touch_other_gap_types():
    test_fail = Gap(
        id="G9", severity="high", gap_type="test_failed", task_id="implement_x",
        description="boom", recommended_fix="fix", blocking=True, acceptance_criterion_id="AC-1",
    )
    gaps = [_ac_gap("G1", "AC-1"), test_fail]
    kept = reconcile_cross_task_acceptance(gaps, proven_acs={"AC-1"})
    # The AC gap is cleared; a real test failure for the same criterion still stands.
    assert [g.id for g in kept] == ["G9"]


def test_ignores_non_blocking_acceptance_gaps():
    gaps = [_ac_gap("G1", "AC-1", blocking=False)]
    # Non-blocking advisory gaps are left as-is (nothing to clear).
    kept = reconcile_cross_task_acceptance(gaps, proven_acs={"AC-1"})
    assert [g.id for g in kept] == ["G1"]
