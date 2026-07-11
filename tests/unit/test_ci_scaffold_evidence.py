import yaml

from devcouncil.cli.commands.init import initialize_project
from devcouncil.repo.ci_scaffold import (
    EVIDENCE_WORKFLOW_RELPATH,
    render_evidence_workflow,
    scaffold_evidence_ci,
)


def _init(tmp_path):
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)


def test_render_evidence_workflow_includes_verify_and_artifacts(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    doc = yaml.safe_load(render_evidence_workflow(tmp_path))
    steps = doc["jobs"]["evidence"]["steps"]
    names = [s.get("name", "") for s in steps]
    assert any("verify" in n.lower() for n in names)
    assert any("evidence JSON" in n for n in names)
    assert any("evidence HTML" in n for n in names)
    assert any("Upload evidence" in n for n in names)


def test_render_evidence_workflow_wires_github_env(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    text = render_evidence_workflow(tmp_path)
    assert "GITHUB_TOKEN" in text
    assert "GITHUB_REPOSITORY" in text
    assert "GITHUB_SHA" in text
    assert "GITHUB_PR_NUMBER" in text
    assert "--github-pr-comment" in text
    assert "--github" in text
    assert "checks: write" in text


def test_render_evidence_workflow_documents_blocking_only_checks(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    text = render_evidence_workflow(tmp_path)
    assert "blocking gaps" in text.lower()


def test_scaffold_evidence_ci_writes_and_respects_force(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
    _init(tmp_path)
    target = scaffold_evidence_ci(tmp_path)
    assert target is not None
    assert target == tmp_path / EVIDENCE_WORKFLOW_RELPATH
    yaml.safe_load(target.read_text(encoding="utf-8"))
    assert scaffold_evidence_ci(tmp_path) is None
    assert scaffold_evidence_ci(tmp_path, force=True) == target
