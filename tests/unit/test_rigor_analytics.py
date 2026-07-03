"""Tests for cross-run rigor analytics."""

import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository
from devcouncil.verification.rigor_analytics import build_rigor_report


def test_build_rigor_report_aggregates_gaps(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="T", description="D"))
        GapRepository(session).save(
            Gap(
                id="G1",
                severity="high",
                gap_type="stub_detected",
                task_id="TASK-1",
                description="stub",
                recommended_fix="fix",
                blocking=True,
            )
        )
        GapRepository(session).save(
            Gap(
                id="G2",
                severity="medium",
                gap_type="stub_declared",
                task_id="TASK-1",
                description="decl",
                recommended_fix="review",
                blocking=False,
            )
        )
    report = build_rigor_report(tmp_path)
    assert report.by_gap_type["stub_detected"] == 1
    assert report.stub_declared_count == 1
    assert "stub_detected" in report.to_markdown()


runner = CliRunner()


def test_cli_report_rigor_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / ".devcouncil").mkdir()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="T", description="D"))
        GapRepository(session).save(
            Gap(
                id="G1",
                severity="high",
                gap_type="stub_detected",
                task_id="TASK-1",
                description="stub",
                recommended_fix="fix",
                blocking=True,
            )
        )

    result = runner.invoke(app, ["report", "rigor", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["by_gap_type"]["stub_detected"] == 1
