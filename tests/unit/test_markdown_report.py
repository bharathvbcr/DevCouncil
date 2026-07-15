"""Unit tests for MarkdownReportGenerator."""


from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.evidence import TestEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import Task
from devcouncil.reporting.markdown_report import MarkdownReportGenerator


def _graph_with_evidence(*modes: str) -> ArtifactGraph:
    graph = ArtifactGraph()
    req = Requirement(
        id="REQ-1",
        title="Feature",
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
        graph.add_test_evidence(
            TestEvidence(
                requirement_id="REQ-1",
                acceptance_criterion_id=f"AC-{i}",
                command="(devcouncil acceptance check)",
                status="passed",
                evidence_summary="proven",
                mode=mode,
            )
        )
    return graph


def test_markdown_report_passed_verdict():
    md = MarkdownReportGenerator.generate(_graph_with_evidence("compiled"))
    assert "**Passed**: Ready for release." in md
    assert "## Coverage Summary" in md
    assert "Proof rigor" in md
    assert "1 compiled" in md


def test_markdown_report_incomplete_verdict():
    graph = ArtifactGraph()
    req = Requirement(
        id="REQ-1",
        title="t",
        description="d",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-0", description="x", verification_method="unit_test"),
            AcceptanceCriterion(id="AC-1", description="y", verification_method="unit_test"),
        ],
    )
    graph.requirements[req.id] = req
    graph.add_test_evidence(
        TestEvidence(
            requirement_id="REQ-1",
            acceptance_criterion_id="AC-0",
            command="(c)",
            status="passed",
            evidence_summary="p",
            mode="vote",
        )
    )
    md = MarkdownReportGenerator.generate(graph)
    assert "**Incomplete**" in md
    assert "1 acceptance criterion(s) lack passing evidence" in md


def test_markdown_report_blocked_by_gaps_and_live_review():
    graph = _graph_with_evidence("compiled")
    graph.add_gap(
        Gap(
            id="G1",
            severity="high",
            gap_type="test_failed",
            task_id="T",
            description="boom",
            recommended_fix="fix",
            blocking=True,
        )
    )
    live = {
        "blocking_cards": [{"id": "CARD-1", "task_id": "TASK-1", "summary": "needs fix"}],
        "pending_signals": 2,
        "cards": {"open": 3, "critical_open": 1},
    }
    md = MarkdownReportGenerator.generate(graph, live_review=live)
    assert "**Blocked**" in md
    assert "1 high-severity gap(s)" in md
    assert "1 live-review blocker(s)" in md
    assert "## Live Review" in md
    assert "CARD-1" in md
    assert "`TASK-1`" in md


def test_markdown_report_blocked_live_review_only():
    graph = _graph_with_evidence("compiled")
    live = {"blocking_cards": [{"id": "CARD-2", "summary": "blocker"}]}
    md = MarkdownReportGenerator.generate(graph, live_review=live)
    assert "**Blocked**" in md
    assert "1 live-review blocker(s)" in md


def test_markdown_report_requirement_matrix_unmapped():
    graph = ArtifactGraph()
    graph.requirements["REQ-1"] = Requirement(
        id="REQ-1",
        title="Lonely",
        description="d",
        priority="high",
        source="user",
        acceptance_criteria=[],
    )
    md = MarkdownReportGenerator.generate(graph)
    assert "*None*" in md
    assert "**Unmapped**" in md


def test_markdown_report_requirement_with_linked_task():
    graph = ArtifactGraph()
    graph.requirements["REQ-1"] = Requirement(
        id="REQ-1",
        title="Mapped",
        description="d",
        priority="high",
        source="user",
        acceptance_criteria=[],
    )
    graph.add_task(
        Task(
            id="TASK-1",
            title="Do it",
            description="d",
            requirement_ids=["REQ-1"],
            status="planned",
        )
    )
    md = MarkdownReportGenerator.generate(graph)
    assert "TASK-1" in md
    assert "planned" in md


def test_markdown_report_acceptance_criteria_table():
    graph = _graph_with_evidence("coarse")
    md = MarkdownReportGenerator.generate(graph)
    assert "## Acceptance Criteria Evidence" in md
    assert "| REQ-1 | AC-0 | passed |" in md


def test_markdown_report_blocking_gaps_none_and_truncation():
    graph = ArtifactGraph()
    md_empty = MarkdownReportGenerator.generate(graph)
    assert "## Blocking Gaps" in md_empty
    assert "None." in md_empty

    for i in range(30):
        graph.add_gap(
            Gap(
                id=f"G{i}",
                severity="high",
                gap_type="test_failed",
                task_id="T",
                description=f"gap {i}",
                recommended_fix="fix",
                blocking=True,
            )
        )
    md = MarkdownReportGenerator.generate(graph)
    assert "Omitted 5 additional blocking gap(s)" in md


def test_markdown_report_live_review_no_blockers():
    graph = ArtifactGraph()
    live = {"blocking_cards": [], "pending_signals": 0, "cards": {"open": 0, "critical_open": 0}}
    md = MarkdownReportGenerator.generate(graph, live_review=live)
    assert "Blocking cards in scope**: None." in md


def test_markdown_report_wiki_refresh_section():
    graph = ArtifactGraph()
    wiki = {
        "considered": True,
        "reason": "stale docs",
        "stale_pages": [f"page-{i}.md" for i in range(12)],
    }
    md = MarkdownReportGenerator.generate(graph, wiki_refresh=wiki)
    assert "## Wiki Refresh" in md
    assert "stale docs" in md
    assert "Stale pages**: 12" in md
    assert "page-0.md" in md


def test_markdown_report_proof_modes_ignores_non_passed_evidence():
    graph = _graph_with_evidence("compiled")
    graph.add_test_evidence(
        TestEvidence(
            requirement_id="REQ-1",
            acceptance_criterion_id="AC-0",
            command="(c)",
            status="failed",
            evidence_summary="nope",
            mode="vote",
        )
    )
    md = MarkdownReportGenerator.generate(graph)
    assert "1 compiled" in md
    assert "vote" not in md.split("Proof rigor")[1].split("\n")[0]
