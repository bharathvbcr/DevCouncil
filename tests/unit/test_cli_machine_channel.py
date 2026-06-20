"""Rank 9 — agent-consumable CLI: --json on prompt/handoff, exit codes on status/report."""

import json

from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import GapRepository, RequirementRepository, TaskRepository

runner = CliRunner()


def _init(tmp_path):
    initialize_project(tmp_path, quiet=True)
    return get_db(tmp_path)


def test_prompt_json_emits_parseable_envelope(tmp_path):
    db = _init(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="Build it", description="do the thing",
            planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
        ))
    result = runner.invoke(app, ["prompt", "TASK-001", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["ok"] is True
    assert payload["task_id"] == "TASK-001"
    assert "Build it" in payload["prompt"]


def test_prompt_json_reports_missing_task(tmp_path):
    _init(tmp_path)
    result = runner.invoke(app, ["prompt", "NOPE", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    payload = json.loads(result.output)
    assert payload["ok"] is False


def _seed_blocking(tmp_path):
    db = _init(tmp_path)
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-001", title="R", description="d", priority="high", source="user",
            acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
        ))
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="d", requirement_ids=["REQ-001"]))
        GapRepository(session).save(Gap(
            id="GAP-1", severity="high", gap_type="test_failed", task_id="TASK-001",
            description="boom", recommended_fix="fix", blocking=True,
        ))


def test_status_fail_on_blocking_exits_nonzero(tmp_path):
    _seed_blocking(tmp_path)
    ok = runner.invoke(app, ["status", "--json", "--project-root", str(tmp_path)])
    assert ok.exit_code == 0  # default: report, don't fail
    gated = runner.invoke(app, ["status", "--json", "--fail-on-blocking", "--project-root", str(tmp_path)])
    assert gated.exit_code == 1


def test_report_fail_on_blocking_exits_nonzero(tmp_path):
    _seed_blocking(tmp_path)
    ok = runner.invoke(app, ["report", "--json", "--project-root", str(tmp_path)])
    assert ok.exit_code == 0
    gated = runner.invoke(app, ["report", "--json", "--fail-on-blocking", "--project-root", str(tmp_path)])
    assert gated.exit_code == 1


def test_handoff_json_envelope(tmp_path):
    db = _init(tmp_path)
    with db.get_session() as session:
        # Handoff requires a task that has been checked out / has a run; ValueError
        # surfaces as a JSON error envelope (still parseable), which is the contract.
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="d"))
    result = runner.invoke(app, [
        "handoff", "TASK-001", "--from", "codex", "--to", "claude",
        "--json", "--project-root", str(tmp_path),
    ])
    payload = json.loads(result.output)
    assert "ok" in payload and payload["task_id"] == "TASK-001"
