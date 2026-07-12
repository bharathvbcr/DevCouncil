import json

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.evidence import TestEvidence
from devcouncil.reporting.json_report import JsonReportGenerator


def _graph_with_evidence(*modes: str) -> ArtifactGraph:
    graph = ArtifactGraph()
    req = Requirement(
        id="REQ-1",
        title="t",
        description="d",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=f"AC-{i}", description="x", verification_method="unit_test")
            for i in range(len(modes))
        ],
    )
    graph.requirements[req.id] = req
    for i, mode in enumerate(modes):
        graph.add_test_evidence(TestEvidence(
            requirement_id="REQ-1",
            acceptance_criterion_id=f"AC-{i}",
            command="(devcouncil acceptance check)",
            status="passed",
            evidence_summary="proven",
            mode=mode,
        ))
    return graph


def test_json_report_surfaces_proof_mode_breakdown():
    report = json.loads(JsonReportGenerator.generate(_graph_with_evidence("vote", "compiled", "coarse")))
    assert report["proof_modes"] == {"vote": 1, "compiled": 1, "coarse": 1}
    assert report["verdict"] == "passed"  # every AC has passing evidence
    assert "requirement_task_matrix" in report
    assert "acceptance_criteria_evidence_matrix" in report
    assert len(report["acceptance_criteria_evidence_matrix"]) == 3


def test_json_report_proof_modes_counts_unspecified_legacy_evidence():
    # Evidence persisted before the mode field defaults to "" -> bucketed as "unspecified",
    # so a report over old runs is still well-formed (no crash, no silent drop).
    report = json.loads(JsonReportGenerator.generate(_graph_with_evidence("")))
    assert report["proof_modes"] == {"unspecified": 1}


def test_json_report_verdict_incomplete_when_an_ac_lacks_evidence():
    # Two ACs, only one proven -> not blocked, but not done either.
    graph = ArtifactGraph()
    req = Requirement(
        id="REQ-1", title="t", description="d", priority="high", source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-0", description="x", verification_method="unit_test"),
            AcceptanceCriterion(id="AC-1", description="y", verification_method="unit_test"),
        ],
    )
    graph.requirements[req.id] = req
    graph.add_test_evidence(TestEvidence(
        requirement_id="REQ-1", acceptance_criterion_id="AC-0",
        command="(c)", status="passed", evidence_summary="p", mode="compiled"))
    report = json.loads(JsonReportGenerator.generate(graph))
    assert report["verdict"] == "incomplete"
    assert report["proof_modes"] == {"compiled": 1}  # failed/missing ACs not counted


def test_json_report_verdict_blocked_when_blocking_gap_present():
    from devcouncil.domain.gap import Gap

    graph = _graph_with_evidence("compiled")  # AC-0 proven...
    graph.add_gap(Gap(
        id="G1", severity="high", gap_type="test_failed", task_id="T",
        description="boom", recommended_fix="fix", blocking=True,
    ))
    report = json.loads(JsonReportGenerator.generate(graph))
    assert report["verdict"] == "blocked"  # a blocking gap dominates proven evidence
