"""CLI surface for `dev okf` and `dev design`."""

import subprocess
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _init_repo(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
    assert runner.invoke(app, ["init"]).exit_code == 0


def _seed_artifacts(root: Path):
    from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    db = get_db(root)
    with db.get_session() as s:
        RequirementRepository(s).save(Requirement(
            id="REQ-001", title="Login", description="Users log in.", priority="high",
            source="user", acceptance_criteria=[AcceptanceCriterion(
                id="AC-1", description="ok", verification_method="unit_test")],
        ))
        TaskRepository(s).save(Task(
            id="TASK-001", title="Impl login", description="Add handler.",
            requirement_ids=["REQ-001"],
            planned_files=[PlannedFile(path="auth.py", reason="h", allowed_change="create")],
        ))


def test_okf_export_validate_ingest_round_trip(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _init_repo(tmp_path)
    _seed_artifacts(tmp_path.resolve())

    export = runner.invoke(app, ["okf", "export", "-o", "bundle"])
    assert export.exit_code == 0, export.output
    assert (tmp_path / "bundle" / "index.md").exists()

    validate = runner.invoke(app, ["okf", "validate", "bundle"])
    assert validate.exit_code == 0
    assert "Valid OKF bundle" in validate.output

    ingest = runner.invoke(app, ["okf", "ingest", "bundle", "--name", "self"])
    assert ingest.exit_code == 0
    assert (tmp_path / ".devcouncil" / "knowledge" / "okf" / "self" / "tasks" / "TASK-001.md").exists()


def test_okf_validate_rejects_broken_bundle(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    broken = tmp_path / "broken"
    broken.mkdir()
    (broken / "a.md").write_text("---\ntitle: no type\n---\n[x](./missing.md)", encoding="utf-8")
    result = runner.invoke(app, ["okf", "validate", "broken"])
    assert result.exit_code == 1
    assert "problem" in result.output.lower()


def test_design_lint_and_export(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _init_repo(tmp_path)
    design_dir = tmp_path / ".devcouncil" / "knowledge" / "design"
    design_dir.mkdir(parents=True)
    (design_dir / "design.md").write_text(
        '---\nname: Acme\ncolors:\n  primary: "#1a1a1a"\n  surface: "#ffffff"\n'
        "components:\n  button:\n    backgroundColor: colors.primary\n    textColor: colors.surface\n"
        "---\n# Overview\no\n",
        encoding="utf-8",
    )
    lint = runner.invoke(app, ["design", "lint"])
    assert lint.exit_code == 0, lint.output

    css = runner.invoke(app, ["design", "export", "--format", "css"])
    assert css.exit_code == 0
    assert "--color-primary: #1a1a1a;" in css.output


def test_design_lint_fails_on_broken_reference(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    bad = tmp_path / "DESIGN.md"
    bad.write_text(
        '---\ncolors:\n  primary: "#000000"\ncomponents:\n  c:\n    backgroundColor: colors.ghost\n---\n# Overview\no\n',
        encoding="utf-8",
    )
    result = runner.invoke(app, ["design", "lint", "DESIGN.md"])
    assert result.exit_code == 1
    assert "broken-token-reference" in result.output
