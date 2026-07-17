import json

import pytest

from devcouncil.app.errors import GatingError
from devcouncil.artifacts.coverage import (
    acceptance_criteria_evidence_matrix,
    can_approve_plan,
    requirement_task_matrix,
)
from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.artifacts.migrations import ArtifactMigrator
from devcouncil.artifacts.schemas import CoverageMatrix, FileModification, ReportSchema
from devcouncil.artifacts.serializer import ArtifactSerializer
from devcouncil.artifacts.validators import ArtifactValidator
from devcouncil.domain.assumption import Assumption
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.domain.evidence import TestEvidence
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task


def _make_ac(id: str = "AC-001", method: str = "unit_test") -> AcceptanceCriterion:
    return AcceptanceCriterion(
        id=id,
        description=f"Verify {id}",
        verification_method=method,
    )


def _make_requirement(
    id: str = "REQ-001",
    *,
    title: str = "Login",
    ac_count: int = 1,
) -> Requirement:
    return Requirement(
        id=id,
        title=title,
        description=f"Description for {id}",
        priority="high",
        source="user",
        acceptance_criteria=[_make_ac(f"{id}-AC-{index}") for index in range(1, ac_count + 1)],
    )


def _make_task(
    id: str = "TASK-001",
    *,
    requirement_ids: list[str] | None = None,
    acceptance_criterion_ids: list[str] | None = None,
    planned_files: list[PlannedFile] | None = None,
    allowed_commands: list[str] | None = None,
    expected_tests: list[str] | None = None,
    status: str = "planned",
) -> Task:
    return Task(
        id=id,
        title=f"Task {id}",
        description=f"Implement {id}",
        requirement_ids=requirement_ids if requirement_ids is not None else ["REQ-001"],
        acceptance_criterion_ids=acceptance_criterion_ids if acceptance_criterion_ids is not None else ["REQ-001-AC-1"],
        planned_files=planned_files
        if planned_files is not None
        else [
            PlannedFile(
                path="src/auth.py",
                reason="Implement auth",
                allowed_change="modify",
            )
        ],
        expected_tests=expected_tests if expected_tests is not None else ["pytest tests/test_auth.py"],
        allowed_commands=allowed_commands if allowed_commands is not None else [],
        status=status,
    )


def test_migrate_requirement_adds_default_priority_in_place():
    data = {"id": "REQ-001", "title": "Login"}

    migrated = ArtifactMigrator.migrate_requirement(data)

    assert migrated is data
    assert migrated["priority"] == "medium"


def test_migrate_requirement_preserves_existing_priority():
    data = {"id": "REQ-001", "title": "Login", "priority": "critical"}

    assert ArtifactMigrator.migrate_requirement(data)["priority"] == "critical"


def test_migrate_task_adds_missing_list_fields_in_place():
    data = {"id": "TASK-001", "title": "Build login"}

    migrated = ArtifactMigrator.migrate_task(data)

    assert migrated is data
    assert migrated["forbidden_changes"] == []
    assert migrated["expected_tests"] == []


def test_migrate_task_preserves_existing_list_fields():
    data = {
        "id": "TASK-001",
        "forbidden_changes": ["src/legacy.py"],
        "expected_tests": ["pytest tests/test_login.py"],
    }

    migrated = ArtifactMigrator.migrate_task(data)

    assert migrated["forbidden_changes"] == ["src/legacy.py"]
    assert migrated["expected_tests"] == ["pytest tests/test_login.py"]


def test_artifact_schema_construction_defaults_and_nested_report():
    modification = FileModification(path="src/app.py", diff="+print('ok')")
    uncovered = CoverageMatrix(requirement_id="REQ-001")
    covered = CoverageMatrix(
        requirement_id="REQ-002",
        task_ids=["TASK-002"],
        test_evidence_ids=["EV-002"],
        is_covered=True,
    )

    report = ReportSchema(
        project_id="project",
        tasks_completed=2,
        tasks_blocked=1,
        open_gaps=0,
        coverage_matrix=[uncovered, covered],
    )

    assert modification.path == "src/app.py"
    assert uncovered.task_ids == []
    assert uncovered.test_evidence_ids == []
    assert uncovered.is_covered is False
    assert covered.is_covered is True
    assert report.coverage_matrix[1].task_ids == ["TASK-002"]


def test_artifact_serializer_round_trips_model_to_json_and_dict():
    requirement = _make_requirement()

    json_payload = ArtifactSerializer.to_json(requirement)
    decoded = json.loads(json_payload)
    restored = ArtifactSerializer.from_json(json_payload, Requirement)
    as_dict = ArtifactSerializer.to_dict(requirement)

    assert "\n" in json_payload
    assert decoded["id"] == "REQ-001"
    assert restored == requirement
    assert as_dict["acceptance_criteria"][0]["verification_method"] == "unit_test"


def test_requirement_task_matrix_marks_unplanned_requirement():
    graph = ArtifactGraph()
    graph.add_requirement(_make_requirement("REQ-001", title="Login"))

    assert requirement_task_matrix(graph) == [
        {
            "requirement": "REQ-001",
            "title": "Login",
            "task": "(none)",
            "task_status": "NOT PLANNED",
        }
    ]


def test_requirement_task_matrix_lists_each_linked_task_status():
    graph = ArtifactGraph()
    graph.add_requirement(_make_requirement("REQ-001", title="Login"))
    graph.add_task(_make_task("TASK-001", requirement_ids=["REQ-001"], status="ready"))
    graph.add_task(_make_task("TASK-002", requirement_ids=["REQ-001"], status="verified"))
    graph.add_task(_make_task("TASK-OTHER", requirement_ids=["REQ-OTHER"]))

    rows = requirement_task_matrix(graph)

    assert rows == [
        {
            "requirement": "REQ-001",
            "title": "Login",
            "task": "TASK-001",
            "task_status": "ready",
        },
        {
            "requirement": "REQ-001",
            "title": "Login",
            "task": "TASK-002",
            "task_status": "verified",
        },
    ]


def test_acceptance_criteria_evidence_matrix_marks_missing_evidence():
    graph = ArtifactGraph()
    graph.add_requirement(_make_requirement("REQ-001", ac_count=2))

    rows = acceptance_criteria_evidence_matrix(graph)

    assert [row["status"] for row in rows] == ["no_evidence", "no_evidence"]
    assert rows[0]["verification_method"] == "unit_test"


def test_acceptance_criteria_evidence_matrix_uses_evidence_status():
    graph = ArtifactGraph()
    graph.add_requirement(_make_requirement("REQ-001", ac_count=2))
    graph.add_test_evidence(
        TestEvidence(
            requirement_id="REQ-001",
            acceptance_criterion_id="REQ-001-AC-1",
            command="pytest tests/test_auth.py",
            status="failed",
            evidence_summary="Regression failed",
        )
    )

    rows = acceptance_criteria_evidence_matrix(graph)

    assert rows[0]["status"] == "failed"
    assert rows[1]["status"] == "no_evidence"


def test_can_approve_plan_passes_when_required_links_exist():
    graph = ArtifactGraph()
    graph.add_requirement(_make_requirement("REQ-001"))
    graph.add_task(_make_task("TASK-001", requirement_ids=["REQ-001"]))

    passed, reasons = can_approve_plan(graph)

    assert passed is True
    assert reasons == []


def test_can_approve_plan_reports_all_gate_failures():
    graph = ArtifactGraph()
    graph.add_requirement(
        Requirement(
            id="REQ-EMPTY",
            title="No AC",
            description="Missing acceptance criteria",
            priority="medium",
            source="user",
            acceptance_criteria=[],
        )
    )
    graph.add_requirement(_make_requirement("REQ-ORPHAN"))
    graph.add_task(_make_task("TASK-ORPHAN", requirement_ids=[]))
    graph.add_assumption(
        Assumption(
            id="ASM-001",
            statement="Payment provider supports refunds",
            confidence="medium",
            impact="high",
            reversible=True,
            requires_user_confirmation=True,
            status="open",
        )
    )
    graph.add_finding(
        CritiqueFinding(
            id="FIND-001",
            severity="critical",
            claim="Missing auth boundary",
            source_agent="critic_a",
            target_plan_id="PLAN-001",
            finding_type="security_risk",
            falsifiable_check="Inspect middleware",
            status="open",
        )
    )

    passed, reasons = can_approve_plan(graph)

    assert passed is False
    assert any("REQ-EMPTY has no acceptance criteria" in reason for reason in reasons)
    assert any("REQ-ORPHAN is not mapped" in reason for reason in reasons)
    assert any("TASK-ORPHAN is not mapped" in reason for reason in reasons)
    assert any("Assumption ASM-001" in reason for reason in reasons)
    assert any("Critical finding FIND-001" in reason for reason in reasons)


def test_validate_requirement_accepts_complete_requirement():
    ArtifactValidator.validate_requirement(_make_requirement())


@pytest.mark.parametrize(
    ("requirement", "message"),
    [
        (_make_requirement(title=""), "missing title"),
        (
            Requirement(
                id="REQ-001",
                title="Login",
                description="No AC",
                priority="medium",
                source="user",
                acceptance_criteria=[],
            ),
            "must have at least one acceptance criterion",
        ),
        (
            Requirement.model_construct(
                id="REQ-001",
                title="Login",
                description="Bad AC",
                priority="medium",
                source="user",
                acceptance_criteria=[
                    AcceptanceCriterion.model_construct(
                        id="AC-001",
                        description="Missing method",
                        verification_method="",
                        required=True,
                    )
                ],
            ),
            "missing verification method",
        ),
    ],
)
def test_validate_requirement_rejects_invalid_requirement_branches(requirement, message):
    with pytest.raises(GatingError, match=message):
        ArtifactValidator.validate_requirement(requirement)


def test_validate_task_accepts_complete_task():
    ArtifactValidator.validate_task(_make_task())


@pytest.mark.parametrize(
    ("task", "message"),
    [
        (_make_task(requirement_ids=[]), "must map to at least one requirement"),
        (_make_task(planned_files=[]), "must have at least one planned file"),
        (_make_task(acceptance_criterion_ids=[]), "must map to at least one acceptance criterion"),
        (_make_task(allowed_commands=[], expected_tests=[]), "must define allowed commands or expected tests"),
    ],
)
def test_validate_task_rejects_invalid_task_branches(task, message):
    with pytest.raises(GatingError, match=message):
        ArtifactValidator.validate_task(task)
