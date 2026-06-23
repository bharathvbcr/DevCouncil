from devcouncil.domain.gap import Gap
from devcouncil.verification.next_actions import build_next_actions, next_action_for


def _gap(**kwargs) -> Gap:
    base = dict(
        id="GAP-1",
        severity="high",
        gap_type="test_failed",
        task_id="TASK-001",
        description="something failed",
        recommended_fix="fix it",
        evidence=[],
        blocking=True,
    )
    base.update(kwargs)
    return Gap(**base)


def test_diff_not_exercised_maps_to_add_test_with_location():
    gap = _gap(
        gap_type="diff_not_exercised",
        description="Tests passed but exercised 0/3 changed lines.",
        file="src/calc.py",
        line=12,
        suggested_command="python -m pytest tests/test_calc.py -q",
        blocking=True,
    )

    action = next_action_for(gap)

    assert action.category == "add_test"
    assert action.file == "src/calc.py"
    assert action.line == 12
    assert action.suggested_command == "python -m pytest tests/test_calc.py -q"
    assert action.missing_evidence == "Tests passed but exercised 0/3 changed lines."
    assert "test that executes the changed lines" in action.action


def test_test_failed_maps_to_fix_code_with_command():
    gap = _gap(gap_type="test_failed", suggested_command="python -m pytest -q")

    action = next_action_for(gap)

    assert action.category == "fix_code"
    assert "python -m pytest -q" in action.action
    assert action.suggested_command == "python -m pytest -q"


def test_orphan_diff_derives_file_from_evidence_when_hint_absent():
    # Simulates a gap reloaded from the DB: no `file` hint, but evidence carries it.
    gap = _gap(gap_type="orphan_diff", evidence=["src/rogue.py"], file=None)

    action = next_action_for(gap)

    assert action.category == "scope"
    assert action.file == "src/rogue.py"
    assert "src/rogue.py" in action.action


def test_build_next_actions_blocking_only_and_ordered():
    gaps = [
        _gap(id="A", gap_type="planned_file_not_changed", severity="medium", blocking=False),
        _gap(id="B", gap_type="orphan_diff", severity="high", blocking=True, evidence=["x.py"]),
        _gap(id="C", gap_type="architecture_drift", severity="critical", blocking=True),
    ]

    actions = build_next_actions(gaps)

    # Only blocking gaps, ordered critical -> high.
    assert [a.gap_id for a in actions] == ["C", "B"]

    all_actions = build_next_actions(gaps, blocking_only=False)
    assert {a.gap_id for a in all_actions} == {"A", "B", "C"}


def test_acceptance_unproven_surfaces_missing_evidence():
    gap = _gap(
        gap_type="acceptance_criteria_unproven",
        description="AC-001 has no passing verification evidence.",
        acceptance_criterion_id="AC-001",
        expected_verification_method="unit_test",
    )

    action = next_action_for(gap)

    assert action.category == "add_test"
    # Concrete missing-evidence: names the criterion AND its expected method instead
    # of echoing the description.
    assert action.acceptance_criterion_id == "AC-001"
    assert action.expected_verification_method == "unit_test"
    assert "AC-001" in action.missing_evidence
    assert "unit_test" in action.missing_evidence
