"""Rank 18 remainder — the post_task hook verifies the active task when enabled."""

import subprocess

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.live.signals import write_signal
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import GapRepository, RequirementRepository, TaskRepository

runner = CliRunner()


def _init_repo(tmp_path, *, verify_on_post_task: bool):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    flag = "true" if verify_on_post_task else "false"
    (dev / "config.yaml").write_text(
        f"project:\n  name: test\nexecution:\n  verify_on_post_task: {flag}\n", encoding="utf-8"
    )
    db = get_db(tmp_path)
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-001", title="R", description="d", priority="high", source="user",
            acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
        ))
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="d", status="running",
            requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001"],
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            expected_tests=["python --version"],
        ))
    # Mark TASK-001 the active task for the hook to resolve.
    write_signal(tmp_path, "claude", {"task_id": "TASK-001"})
    return db


def test_post_task_default_only_reminds(tmp_path, monkeypatch):
    _init_repo(tmp_path, verify_on_post_task=False)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    result = runner.invoke(app, ["hook", "post-task", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "dev verify" in result.output


def test_post_task_verifies_when_enabled_and_blocks_empty_diff(tmp_path, monkeypatch):
    db = _init_repo(tmp_path, verify_on_post_task=True)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    # No diff was produced, so verification must block (empty-diff guard) and record a gap.
    result = runner.invoke(app, ["hook", "post-task", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "blocked" in result.output.lower()

    with db.get_session() as session:
        gaps = [g for g in GapRepository(session).get_all() if g.task_id == "TASK-001"]
        task = TaskRepository(session).get_by_id("TASK-001")
    assert any(g.blocking for g in gaps)
    assert task.status == "blocked"
