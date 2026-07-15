"""OKF export conformance: artifact graph bundles + code-graph OKF/GraphML."""

from __future__ import annotations

import subprocess

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.evidence import DiffEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.indexing.graph.build import build_code_graph
from devcouncil.indexing.graph.export import (
    export_graphml,
    file_doc_rel,
)
from devcouncil.indexing.graph.export_links import (
    GRAPH_FROM_WIKI,
    file_doc_path,
    relative_md_link,
    wired_to_bullets,
)
from devcouncil.indexing.graph.okf_export import export_graph_okf, graph_to_graphml
from devcouncil.knowledge.okf import OKFDocument, read_bundle, validate_bundle
from devcouncil.knowledge.frontmatter import split_frontmatter
from devcouncil.reporting.report_builder import ReportBuilder


def _artifact_graph() -> ArtifactGraph:
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
    written = ReportBuilder.build_okf_bundle(_artifact_graph(), out, project_name="Demo")
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
    ReportBuilder.build_okf_bundle(_artifact_graph(), out)
    task = read_bundle(out).by_path()["tasks/TASK-001.md"]
    assert "requirements/REQ-001.md" in task.links
    assert "evidence/TASK-001-diff-0.md" in task.links
    assert "gaps/GAP-001.md" in task.links


def test_requirement_frontmatter_carries_priority_tag(tmp_path):
    out = tmp_path / "bundle"
    ReportBuilder.build_okf_bundle(_artifact_graph(), out)
    req = read_bundle(out).by_path()["requirements/REQ-001.md"]
    assert req.type == "DevCouncil Requirement"
    assert "high" in req.tags


def test_empty_graph_still_emits_root_and_section_indexes(tmp_path):
    out = tmp_path / "bundle"
    ReportBuilder.build_okf_bundle(ArtifactGraph(), out)
    bundle = read_bundle(out)
    assert "index.md" in bundle.by_path()
    assert validate_bundle(bundle) == []


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def _assert_frontmatter_fields(text: str) -> dict:
    meta, _body = split_frontmatter(text)
    assert "type" in meta and str(meta["type"]).strip()
    # OKF v0.1 producer fields DevCouncil emits
    for key in ("title", "description", "tags", "timestamp"):
        assert key in meta, f"missing frontmatter field {key}"
    return meta


def test_code_graph_okf_frontmatter_and_indexes(tmp_path):
    _write(
        tmp_path,
        {
            "pkg/__init__.py": "",
            "pkg/a.py": "def entry():\n    return helper()\n\ndef helper():\n    return 1\n",
            "pkg/b.py": "from pkg import a\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    # Enrich community when present; fallback path still works without it
    for n in graph.nodes:
        if n.path == "pkg/a.py" and n.kind.value == "file":
            n.community = "demo-community"
    out = tmp_path / "okf-graph"
    written = export_graph_okf(graph, out, root=tmp_path, project_name="DemoGraph")
    assert written
    bundle = read_bundle(out)
    paths = set(bundle.by_path())
    assert "index.md" in paths
    assert "files/index.md" in paths
    assert "subsystems/index.md" in paths
    assert file_doc_rel("pkg/a.py") in paths
    assert file_doc_path("pkg/a.py") == file_doc_rel("pkg/a.py")

    # Frontmatter conformance on every document
    for rel, doc in bundle.by_path().items():
        raw = (out / rel).read_text(encoding="utf-8")
        meta = _assert_frontmatter_fields(raw)
        assert doc.type == str(meta["type"])
        # Round-trip through OKFDocument
        parsed = OKFDocument.from_markdown(raw, rel_path=rel)
        assert parsed.type

    assert validate_bundle(bundle) == []

    # Markdown import/call links present on file pages
    a_doc = bundle.by_path()[file_doc_rel("pkg/a.py")]
    assert "Imports" in a_doc.body or "imports" in a_doc.body.lower() or "Calls" in a_doc.body


def test_graphml_includes_attributes(tmp_path):
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/a.py": "def f():\n    return 1\n"})
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    xml = graph_to_graphml(graph)
    assert 'attr.name="kind"' in xml
    assert 'attr.name="confidence"' in xml
    assert 'attr.name="area"' in xml
    assert 'attr.name="community"' in xml
    assert 'attr.name="dead"' in xml
    assert export_graphml(graph).startswith("<?xml")


def test_wiki_graph_share_link_conventions():
    bullets = wired_to_bullets(
        ["src/foo.py", "src/bar.py"],
        from_rel="subsystems/indexing.md",
        link_to_graph=True,
    )
    assert any(GRAPH_FROM_WIKI in b for b in bullets)
    assert any("files/src/foo.py.md" in b for b in bullets)
    link = relative_md_link("files/index.md", "files/pkg/a.py.md", "pkg/a.py")
    assert link.startswith("[pkg/a.py](")
