from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.reporting.github_check import GitHubCheckGenerator


def test_github_check_success_when_only_advisory_gaps():
    graph = ArtifactGraph()
    req = Requirement(
        id="REQ-1", title="t", description="d", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-1", description="x", verification_method="unit_test")],
    )
    graph.requirements[req.id] = req
    graph.add_gap(Gap(
        id="G1", severity="medium", gap_type="suspicious_effort", task_id="T",
        description="advisory only", recommended_fix="review", blocking=False,
    ))
    payload = GitHubCheckGenerator.generate(graph)
    assert payload["conclusion"] == "success"


def test_github_check_failure_when_blocking_gap():
    graph = ArtifactGraph()
    graph.add_gap(Gap(
        id="G1", severity="high", gap_type="test_failed", task_id="T",
        description="blocking", recommended_fix="fix", blocking=True,
    ))
    payload = GitHubCheckGenerator.generate(graph)
    assert payload["conclusion"] == "failure"
