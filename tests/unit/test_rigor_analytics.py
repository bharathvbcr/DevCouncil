"""Tests for cross-run rigor analytics."""

import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository
from devcouncil.verification.rigor_analytics import RigorAnalyticsReport, build_rigor_report, _manifest_attempts


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


def test_build_rigor_report_no_db_notes(tmp_path):
    report = build_rigor_report(tmp_path)
    assert "No DevCouncil state database" in report.notes[0]


def test_manifest_attempts_reads_correction_manifests(tmp_path):
    runs = tmp_path / ".devcouncil" / "runs" / "run-1"
    runs.mkdir(parents=True)
    (runs / "correction-manifest.json").write_text(
        json.dumps({"prior_failed_attempts": 2}),
        encoding="utf-8",
    )
    (tmp_path / ".devcouncil" / "runs" / "bad").mkdir()
    (tmp_path / ".devcouncil" / "runs" / "bad" / "correction-manifest.json").write_text("not json", encoding="utf-8")

    assert _manifest_attempts(tmp_path) == [3]


def test_build_rigor_report_recurring_stub_tasks_and_manifests(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="T", description="D"))
        for i in range(2):
            GapRepository(session).save(
                Gap(
                    id=f"G{i}",
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
                id="G-eff",
                severity="medium",
                gap_type="suspicious_effort",
                task_id="TASK-1",
                description="effort",
                recommended_fix="review",
                blocking=False,
            )
        )
        task = TaskRepository(session).get_by_id("TASK-1")
        task.status = "verified"
        TaskRepository(session).save(task)

    runs = tmp_path / ".devcouncil" / "runs" / "run-1"
    runs.mkdir(parents=True)
    (runs / "correction-manifest.json").write_text(
        json.dumps({"prior_failed_attempts": 1}),
        encoding="utf-8",
    )

    report = build_rigor_report(tmp_path)
    assert "TASK-1" in report.recurring_stub_tasks
    assert report.tasks_with_repair_attempts == 1
    assert report.avg_repair_attempts == 2.0
    assert any("suspicious_effort advisory" in n for n in report.notes)
    assert any("stub_detected more than once" in n for n in report.notes)

    md = report.to_markdown()
    assert "Tasks recurring on stub gaps" in md
    assert "## Notes" in md


def test_rigor_report_to_markdown_empty_notes():
    report = RigorAnalyticsReport(total_gaps=0)
    md = report.to_markdown()
    assert "Total gaps recorded: **0**" in md


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
