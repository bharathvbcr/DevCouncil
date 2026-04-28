"""Tests for the artifact graph module."""

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.requirement import Requirement, AcceptanceCriterion
from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap
from devcouncil.domain.assumption import Assumption
from devcouncil.domain.evidence import TestEvidence
from devcouncil.domain.critique import CritiqueFinding


def _make_requirement(id: str, title: str, ac_count: int = 1) -> Requirement:
    ac_list = [
        AcceptanceCriterion(
            id=f"{id}-AC-{i+1}",
            description=f"AC {i+1} for {id}",
            verification_method="unit_test",
        )
        for i in range(ac_count)
    ]
    return Requirement(
        id=id,
        title=title,
        description=f"Description for {id}",
        priority="high",
        source="user",
        acceptance_criteria=ac_list,
    )


def _make_task(id: str, req_ids: list[str], status: str = "planned") -> Task:
    return Task(
        id=id,
        title=f"Task {id}",
        description=f"Implement {id}",
        requirement_ids=req_ids,
        planned_files=[],
        status=status,
    )


class TestArtifactGraph:
    def test_empty_graph(self):
        g = ArtifactGraph()
        assert g.coverage_summary()["total_requirements"] == 0
        assert g.coverage_summary()["total_tasks"] == 0

    def test_add_and_query_requirements(self):
        g = ArtifactGraph()
        req = _make_requirement("REQ-001", "Login", ac_count=2)
        g.add_requirement(req)
        assert len(g.requirements) == 1
        assert g.requirements["REQ-001"].title == "Login"

    def test_requirements_without_tasks(self):
        g = ArtifactGraph()
        g.add_requirement(_make_requirement("REQ-001", "Login"))
        g.add_requirement(_make_requirement("REQ-002", "Dashboard"))
        g.add_task(_make_task("TASK-001", ["REQ-001"]))

        orphan_reqs = g.requirements_without_tasks()
        assert len(orphan_reqs) == 1
        assert orphan_reqs[0].id == "REQ-002"

    def test_tasks_without_requirements(self):
        g = ArtifactGraph()
        g.add_task(_make_task("TASK-001", []))  # No requirement links
        assert len(g.tasks_without_requirements()) == 1

    def test_requirements_without_acceptance_criteria(self):
        g = ArtifactGraph()
        g.add_requirement(Requirement(
            id="REQ-BARE", title="Bare requirement",
            description="No AC", priority="medium", source="user",
            acceptance_criteria=[],
        ))
        assert len(g.requirements_without_acceptance_criteria()) == 1

    def test_acceptance_criteria_without_evidence(self):
        g = ArtifactGraph()
        req = _make_requirement("REQ-001", "Login", ac_count=2)
        g.add_requirement(req)

        # Add evidence for only one AC
        g.add_test_evidence(TestEvidence(
            requirement_id="REQ-001",
            acceptance_criterion_id="REQ-001-AC-1",
            command="pytest tests/test_login.py",
            status="passed",
            evidence_summary="All login tests passed.",
        ))

        uncovered = g.acceptance_criteria_without_evidence()
        assert len(uncovered) == 1
        assert uncovered[0][1].id == "REQ-001-AC-2"

    def test_blocking_gaps(self):
        g = ArtifactGraph()
        g.add_gap(Gap(
            id="GAP-001", severity="high", gap_type="orphan_diff",
            task_id="T1", description="Orphan",
            recommended_fix="Revert the file.", blocking=True,
        ))
        g.add_gap(Gap(
            id="GAP-002", severity="low", gap_type="planned_file_not_changed",
            task_id="T1", description="Style issue",
            recommended_fix="Update style.", blocking=False,
        ))
        assert len(g.blocking_gaps()) == 1

    def test_coverage_summary(self):
        g = ArtifactGraph()
        g.add_requirement(_make_requirement("REQ-001", "Auth", ac_count=2))
        g.add_task(_make_task("TASK-001", ["REQ-001"]))

        summary = g.coverage_summary()
        assert summary["total_requirements"] == 1
        assert summary["total_tasks"] == 1
        assert summary["requirements_without_tasks"] == 0
        assert summary["total_ac"] == 2
        assert summary["ac_without_evidence"] == 2

    def test_open_findings(self):
        g = ArtifactGraph()
        g.add_finding(CritiqueFinding(
            id="F-001", severity="critical", claim="Missing auth",
            source_agent="critic_a", target_plan_id="PLAN-A",
            finding_type="security_risk",
            falsifiable_check="Check if auth middleware exists",
            status="open",
        ))
        g.add_finding(CritiqueFinding(
            id="F-002", severity="low", claim="Typo in docs",
            source_agent="critic_b", target_plan_id="PLAN-B",
            finding_type="missing_requirement",
            falsifiable_check="Check docs for typos",
            status="open",
        ))
        assert len(g.open_findings("critical")) == 1
        assert len(g.open_findings("low")) == 2


class TestCoverage:
    def test_can_approve_plan_passes(self):
        from devcouncil.artifacts.coverage import can_approve_plan
        g = ArtifactGraph()
        g.add_requirement(_make_requirement("REQ-001", "Auth"))
        g.add_task(_make_task("TASK-001", ["REQ-001"]))
        passed, reasons = can_approve_plan(g)
        assert passed
        assert len(reasons) == 0

    def test_can_approve_plan_fails_with_orphan_req(self):
        from devcouncil.artifacts.coverage import can_approve_plan
        g = ArtifactGraph()
        g.add_requirement(_make_requirement("REQ-001", "Auth"))
        # No task for REQ-001
        passed, reasons = can_approve_plan(g)
        assert not passed
        assert any("REQ-001" in r for r in reasons)

    def test_can_approve_plan_fails_with_unconfirmed_assumption(self):
        from devcouncil.artifacts.coverage import can_approve_plan
        g = ArtifactGraph()
        g.add_requirement(_make_requirement("REQ-001", "Auth"))
        g.add_task(_make_task("TASK-001", ["REQ-001"]))
        g.add_assumption(Assumption(
            id="ASM-001", statement="DB supports JSON",
            confidence="medium", impact="high",
            reversible=True, requires_user_confirmation=True,
            status="open",
        ))
        passed, reasons = can_approve_plan(g)
        assert not passed
        assert any("ASM-001" in r for r in reasons)
