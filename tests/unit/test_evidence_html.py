import json

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.evidence import TestEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import Task
from devcouncil.reporting.evidence_html import EvidenceHtmlGenerator


def _sample_graph() -> ArtifactGraph:
    graph = ArtifactGraph()
    req = Requirement(
        id="REQ-1",
        title="Login",
        description="User can sign in",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-1", description="valid creds", verification_method="unit_test"),
        ],
    )
    graph.requirements[req.id] = req
    graph.tasks["TASK-1"] = Task(
        id="TASK-1",
        title="Implement login",
        description="d",
        requirement_ids=["REQ-1"],
        acceptance_criterion_ids=["AC-1"],
    )
    graph.add_test_evidence(
        TestEvidence(
            requirement_id="REQ-1",
            acceptance_criterion_id="AC-1",
            command="pytest tests/test_login.py",
            status="passed",
            evidence_summary="ok",
            mode="compiled",
        )
    )
    graph.add_gap(
        Gap(
            id="G-ADV",
            severity="medium",
            gap_type="suspicious_effort",
            task_id="TASK-1",
            description="Consider edge case",
            recommended_fix="add test",
            blocking=False,
        )
    )
    graph.add_gap(
        Gap(
            id="G-BLOCK",
            severity="high",
            gap_type="test_failed",
            task_id="TASK-1",
            description="Missing coverage",
            recommended_fix="run tests",
            blocking=True,
        )
    )
    return graph


def test_evidence_html_includes_ac_table_and_task_links():
    html = EvidenceHtmlGenerator.generate(_sample_graph())
    assert "<table" in html
    assert "AC-1" in html
    assert 'id="task-TASK-1"' in html
    assert 'href="#task-TASK-1"' in html
    assert "pytest tests/test_login.py" in html


def test_evidence_html_separates_blocking_and_advisory_gaps():
    html = EvidenceHtmlGenerator.generate(_sample_graph())
    assert "G-BLOCK" in html
    assert "G-ADV" in html
    assert "blocking" in html
    assert "advisory" in html
    assert "GitHub Checks fail only on blocking gaps" in html


def test_evidence_html_verdict_blocked_when_blocking_gap():
    html = EvidenceHtmlGenerator.generate(_sample_graph())
    assert "verdict-blocked" in html
    assert "Verdict:" in html


def test_evidence_html_is_self_contained():
    html = EvidenceHtmlGenerator.generate(_sample_graph())
    assert html.startswith("<!DOCTYPE html>")
    assert "<style>" in html
    assert "<script" not in html


def test_evidence_html_matches_export_payload():
    graph = _sample_graph()
    from devcouncil.reporting.evidence_export import EvidenceExportGenerator

    export = json.loads(EvidenceExportGenerator.generate(graph))
    html = EvidenceHtmlGenerator.generate(graph)
    assert export["verdict"] == "blocked"
    assert export["requirements"][0]["acceptance_criteria"][0]["id"] in html
