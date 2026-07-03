"""Tests for the effort/diff plausibility heuristics."""

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.effort_heuristics import detect_effort_anomalies


def _task(planned=None, ac_ids=None, expected_tests=None) -> Task:
    return Task(
        id="TASK-001",
        title="Do work",
        description="Implement things.",
        planned_files=planned or [],
        acceptance_criterion_ids=ac_ids or [],
        expected_tests=expected_tests or [],
    )


def _req_with_unit_test_ac(ac_id="AC-1") -> Requirement:
    return Requirement(
        id="REQ-001",
        title="R",
        description="d",
        priority="medium",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=ac_id, description="works", verification_method="unit_test")
        ],
    )


def _diff(path: str, added: list[str]) -> str:
    body = "\n".join(f"+{line}" for line in added)
    return (
        f"--- a/{path}\n"
        f"+++ b/{path}\n"
        f"@@ -0,0 +1,{len(added)} @@\n"
        f"{body}\n"
    )


def _deletion_diff(path: str, removed: list[str], added: list[str] | None = None) -> str:
    lines = [f"-{line}" for line in removed] + [f"+{line}" for line in (added or [])]
    return (
        f"--- a/{path}\n"
        f"+++ b/{path}\n"
        f"@@ -1,{len(removed)} +1,{len(added or [])} @@\n"
        + "\n".join(lines)
        + "\n"
    )


class TestUndersizedDiff:
    def test_flags_tiny_diff_for_big_scope(self):
        planned = [
            PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
            for i in range(4)
        ]
        diff = _diff("src/f0.py", ["x = 1"])
        findings = detect_effort_anomalies(_task(planned=planned), diff)
        assert any(f.reason == "undersized_diff" for f in findings)

    def test_small_scope_not_flagged(self):
        planned = [PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")]
        diff = _diff("src/a.py", ["x = 1"])
        findings = detect_effort_anomalies(_task(planned=planned), diff)
        assert not any(f.reason == "undersized_diff" for f in findings)

    def test_adequate_diff_not_flagged(self):
        planned = [
            PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
            for i in range(3)
        ]
        added = [f"line_{i} = {i}" for i in range(20)]
        diff = _diff("src/f0.py", added)
        findings = detect_effort_anomalies(_task(planned=planned), diff)
        assert not any(f.reason == "undersized_diff" for f in findings)

    def test_declared_stub_lines_do_not_satisfy_undersized_threshold(self):
        planned = [
            PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
            for i in range(4)
        ]
        stub_lines = [f"pass  # devcouncil: allow-stub {i}" for i in range(30)]
        diff = _diff("src/f0.py", stub_lines)
        findings = detect_effort_anomalies(_task(planned=planned), diff)
        assert any(f.reason == "undersized_diff" for f in findings)

    def test_empty_diff_returns_nothing(self):
        planned = [
            PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
            for i in range(4)
        ]
        assert detect_effort_anomalies(_task(planned=planned), "") == []


class TestCommentOnlyDiff:
    def test_flags_comment_only_diff_with_automatable_acs(self):
        task = _task(
            planned=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
            ac_ids=["AC-1"],
        )
        diff = _diff("src/a.py", ["# just a comment", "", "// another comment"])
        findings = detect_effort_anomalies(task, diff, [_req_with_unit_test_ac()])
        assert any(f.reason == "comment_only_diff" for f in findings)

    def test_no_acs_means_no_comment_only_gap(self):
        task = _task(planned=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")])
        diff = _diff("src/a.py", ["# just a comment"])
        findings = detect_effort_anomalies(task, diff, [])
        assert not any(f.reason == "comment_only_diff" for f in findings)


class TestTestDeletion:
    def test_flags_net_test_deletion_when_expected_tests_reference_file(self):
        task = _task(expected_tests=["python -m pytest tests/test_a.py -q"])
        diff = _deletion_diff(
            "tests/test_a.py",
            removed=["def test_x():", "    assert f() == 1", "def test_y():", "    assert g() == 2"],
            added=["def test_x():"],
        )
        findings = detect_effort_anomalies(task, diff)
        deletion = [f for f in findings if f.reason == "test_deletion"]
        assert deletion and deletion[0].severity == "high"
        assert deletion[0].file == "tests/test_a.py"

    def test_test_deletion_not_flagged_without_expected_test_reference(self):
        diff = _deletion_diff(
            "tests/test_a.py",
            removed=["def test_x():", "    assert f() == 1", "def test_y():", "    assert g() == 2"],
            added=["def test_x():"],
        )
        findings = detect_effort_anomalies(_task(), diff)
        assert not any(f.reason == "test_deletion" for f in findings)

    def test_test_refactor_with_net_addition_not_flagged(self):
        diff = _deletion_diff(
            "tests/test_a.py",
            removed=["def test_old():"],
            added=["def test_new():", "    assert f() == 1"],
        )
        findings = detect_effort_anomalies(_task(), diff)
        assert not any(f.reason == "test_deletion" for f in findings)

    def test_non_test_deletion_not_flagged(self):
        diff = _deletion_diff("src/a.py", removed=["x = 1", "y = 2"], added=["z = 3"])
        findings = detect_effort_anomalies(_task(), diff)
        assert not any(f.reason == "test_deletion" for f in findings)
