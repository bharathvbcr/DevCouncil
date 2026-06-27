"""Export the artifact graph as an OKF bundle and verify structure + cross-links."""

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.evidence import DiffEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.knowledge.okf import read_bundle, validate_bundle
from devcouncil.reporting.report_builder import ReportBuilder


def _graph() -> ArtifactGraph:
    g = ArtifactGraph()
    g.add_requirement(Requirement(
        id="REQ-001", title="Login", description="Users can log in.",
        priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(
            id="AC-1", description="valid creds succeed", verification_method="unit_test")],
    ))
    g.add_task(Task(
        id="TASK-001", title="Implement login", description="Add login handler.",
        requirement_ids=["REQ-001"],
        planned_files=[PlannedFile(path="auth.py", reason="handler", allowed_change="create")],
        status="verified",
    ))
    g.add_diff_evidence(DiffEvidence(
        task_id="TASK-001", changed_files=["auth.py"], added_files=["auth.py"],
        deleted_files=[], diff_summary="Added login handler.",
    ))
    g.add_gap(Gap(
        id="GAP-001", severity="high", gap_type="missing_test", task_id="TASK-001",
        requirement_id="REQ-001", description="No test for login.",
        recommended_fix="Add a unit test.", blocking=True,
    ))
    return g


def test_export_produces_valid_linked_bundle(tmp_path):
    out = tmp_path / "bundle"
    written = ReportBuilder.build_okf_bundle(_graph(), out, project_name="Demo")
    assert written

    bundle = read_bundle(out)
    paths = set(bundle.by_path())
    assert "index.md" in paths
    assert "requirements/REQ-001.md" in paths
    assert "tasks/TASK-001.md" in paths
    assert "gaps/GAP-001.md" in paths
    assert "evidence/TASK-001-diff-0.md" in paths

    # Every document is typed and every cross-link resolves — the OKF invariant.
    assert validate_bundle(bundle) == []


def test_task_document_links_requirement_evidence_and_gap(tmp_path):
    out = tmp_path / "bundle"
    ReportBuilder.build_okf_bundle(_graph(), out)
    task = read_bundle(out).by_path()["tasks/TASK-001.md"]
    assert "requirements/REQ-001.md" in task.links
    assert "evidence/TASK-001-diff-0.md" in task.links
    assert "gaps/GAP-001.md" in task.links


def test_requirement_frontmatter_carries_priority_tag(tmp_path):
    out = tmp_path / "bundle"
    ReportBuilder.build_okf_bundle(_graph(), out)
    req = read_bundle(out).by_path()["requirements/REQ-001.md"]
    assert req.type == "DevCouncil Requirement"
    assert "high" in req.tags


def test_empty_graph_still_emits_root_and_section_indexes(tmp_path):
    out = tmp_path / "bundle"
    ReportBuilder.build_okf_bundle(ArtifactGraph(), out)
    bundle = read_bundle(out)
    assert "index.md" in bundle.by_path()
    assert validate_bundle(bundle) == []
