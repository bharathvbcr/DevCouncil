"""Batch A — verification-trust guarantees.

These lock in the fixes that stop an autonomous agent from converging on a false
"done": an empty diff can no longer pass, a single passing command can no longer
"prove" criteria against zero changes, the run reports the rigor it actually ran
at, and only *passing* evidence counts toward acceptance coverage.
"""

import asyncio

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.evidence import CommandResult, TestEvidence
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.next_actions import split_next_actions
from devcouncil.verification.verifier import Verifier


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Password reset",
        description="Reset tokens are single use",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-001", description="Token reuse is rejected", verification_method="unit_test")
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement reset token",
        description="Implement reset token rules",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[PlannedFile(path="src/auth.py", reason="token logic", allowed_change="modify")],
        allowed_commands=["pytest tests/test_auth.py"],
    )


def _passing_verifier(tmp_path, *, diff: str) -> Verifier:
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"] if diff else []
    verifier.get_diff = lambda: diff
    verifier._load_commands = lambda: {"test": ["pytest tests/test_auth.py"], "lint": [], "typecheck": []}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed"
    )
    return verifier


def test_empty_diff_blocks_when_change_expected(tmp_path):
    # A no-op run whose unrelated command happens to pass must NOT pass: the task
    # declared a file to modify but produced no diff.
    verifier = _passing_verifier(tmp_path, diff="")

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    nodiff = [g for g in gaps if g.gap_type == "task_not_implemented"]
    assert nodiff and nodiff[0].blocking
    # And no criterion is "proven" by the coarse fallback against zero changes.
    assert not [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.status == "passed"]
    assert verifier.last_outcome is not None and verifier.last_outcome.diff_empty is True


def test_passing_command_with_real_diff_still_proves(tmp_path):
    # The guard must not regress the happy path: a passing command on a real diff
    # proves the criterion (coarse mode).
    verifier = _passing_verifier(tmp_path, diff="diff --git a/src/auth.py b/src/auth.py\n+token logic")

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert not [g for g in gaps if g.gap_type == "task_not_implemented"]
    assert not [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.blocking]
    assert [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.status == "passed"]
    assert verifier.last_outcome.mode == "coarse"
    assert verifier.last_outcome.diff_empty is False


def test_committed_work_with_empty_working_diff_is_not_blocked(tmp_path):
    # Reconciliation case: `git diff HEAD` is empty because the task was committed,
    # but the task's checkpoint shows it produced work — it must NOT be flagged as
    # an empty/unimplemented task, and a passing command still proves the criterion.
    verifier = _passing_verifier(tmp_path, diff="")
    verifier._task_produced_changes = lambda task_id: True

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert not [g for g in gaps if g.gap_type == "task_not_implemented"]
    assert [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.status == "passed"]


def test_clean_working_tree_falls_back_to_committed_diff(tmp_path):
    # The 0-evidence false-block bug: dev go commits a task between repair attempts, so
    # the working-tree diff is empty at re-verify. Verification must fall back to the
    # COMMITTED checkpoint diff so acceptance compilation/review run (diff_empty False)
    # instead of skipping and marking every criterion unproven → wrongly blocking.
    verifier = _passing_verifier(tmp_path, diff="")  # empty working-tree diff
    verifier._committed_task_diff = lambda task_id: (
        "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    )

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert verifier.last_outcome is not None and verifier.last_outcome.diff_empty is False
    assert not [g for g in gaps if g.gap_type == "task_not_implemented"]
    assert [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.status == "passed"]


def test_outcome_reports_compiled_mode_when_router_present(tmp_path):
    class _Reviewer:
        async def review_changes(self, *a, **k):
            class _R:
                findings = []
            return _R()

    class _Compiler:
        async def compile(self, *a, **k):
            return {}

    verifier = _passing_verifier(tmp_path, diff="diff --git a/src/auth.py b/src/auth.py\n+x")
    verifier.reviewer = _Reviewer()
    verifier.acceptance_compiler = _Compiler()

    asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert verifier.last_outcome.mode == "compiled"


def test_split_next_actions_separates_advisory(tmp_path):
    from devcouncil.domain.gap import Gap

    gaps = [
        Gap(id="G1", severity="high", gap_type="orphan_diff", task_id="T", description="d",
            recommended_fix="f", blocking=True),
        Gap(id="G2", severity="medium", gap_type="diff_not_exercised", task_id="T", description="d",
            recommended_fix="f", blocking=False),
    ]
    blocking, advisory = split_next_actions(gaps)
    assert [a.gap_id for a in blocking] == ["G1"]
    assert [a.gap_id for a in advisory] == ["G2"]


def test_graph_ac_coverage_ignores_non_passing_evidence():
    graph = ArtifactGraph()
    graph.add_requirement(_requirement())
    # A failed check must NOT count as evidence — the criterion stays unproven.
    graph.add_test_evidence(TestEvidence(
        requirement_id="REQ-001", acceptance_criterion_id="AC-001",
        command="pytest", status="failed", evidence_summary="boom",
    ))
    unproven = {ac.id for _req, ac in graph.acceptance_criteria_without_evidence()}
    assert "AC-001" in unproven

    graph.add_test_evidence(TestEvidence(
        requirement_id="REQ-001", acceptance_criterion_id="AC-001",
        command="pytest", status="passed", evidence_summary="ok",
    ))
    unproven_after = {ac.id for _req, ac in graph.acceptance_criteria_without_evidence()}
    assert "AC-001" not in unproven_after


def _req_with_ac(method, required=True):
    return Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-X", description="desc",
                                                 verification_method=method, required=required)],
    )


def _task_ac():
    return Task(
        id="TASK-001", title="T", description="D", requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-X"],
        planned_files=[PlannedFile(path="src/auth.py", reason="x", allowed_change="modify")],
    )


def _unproven_verifier(tmp_path):
    # Work present (diff), but no expected_tests/commands and no compiler -> AC unproven.
    v = Verifier(tmp_path)
    v.get_changed_files = lambda: ["src/auth.py"]
    v.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    v._load_commands = lambda: {"test": [], "lint": [], "typecheck": []}
    return v


def test_manual_ac_unproven_is_advisory_not_blocking(tmp_path):
    gaps, _ = asyncio.run(_unproven_verifier(tmp_path).verify_task(_task_ac(), [_req_with_ac("manual")]))
    ac_gaps = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.acceptance_criterion_id == "AC-X"]
    assert ac_gaps and ac_gaps[0].blocking is False  # manual -> human review, not a gate block


def test_behavioral_ac_unproven_still_blocks(tmp_path):
    gaps, _ = asyncio.run(_unproven_verifier(tmp_path).verify_task(_task_ac(), [_req_with_ac("unit_test")]))
    ac_gaps = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.acceptance_criterion_id == "AC-X"]
    assert ac_gaps and ac_gaps[0].blocking is True  # behavioral criterion must still block when unproven


def test_optional_ac_unproven_is_advisory(tmp_path):
    gaps, _ = asyncio.run(_unproven_verifier(tmp_path).verify_task(_task_ac(), [_req_with_ac("unit_test", required=False)]))
    ac_gaps = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.acceptance_criterion_id == "AC-X"]
    assert ac_gaps and ac_gaps[0].blocking is False  # optional -> advisory


def test_static_check_ac_unproven_is_advisory(tmp_path):
    # Quality-only criteria (PEP 8 / docstrings / formatting) must NOT hard-block the
    # autonomous loop when unproven: they aren't behavioral correctness, and the compiler
    # frequently can't author a reliable style check (or the criterion is mis-assigned to
    # a no-diff process task), which otherwise false-blocks correct, conforming code.
    gaps, _ = asyncio.run(_unproven_verifier(tmp_path).verify_task(_task_ac(), [_req_with_ac("static_check")]))
    ac_gaps = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.acceptance_criterion_id == "AC-X"]
    assert ac_gaps and ac_gaps[0].blocking is False  # static_check -> advisory, not a gate block
