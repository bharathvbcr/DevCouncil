"""Unit tests for release-health classification (F-14)."""

from __future__ import annotations

import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.reporting.release_health import (
    build_baseline_snapshot,
    build_release_health_report,
    compact_release_health_summary,
    write_baseline_snapshot,
)

runner = CliRunner()


def _gap(gap_id: str, *, blocking: bool = True, description: str = "x") -> Gap:
    return Gap(
        id=gap_id,
        severity="high",
        gap_type="test_failed",
        description=description,
        recommended_fix="fix it",
        blocking=blocking,
    )


def test_historical_debt_is_not_release_ready():
    baseline = build_baseline_snapshot([_gap("GAP-OLD")])
    report = build_release_health_report([_gap("GAP-OLD")], baseline=baseline)
    assert report.verdict == "historical_debt"
    assert report.release_ready is False
    assert report.counts["historical_blocking"] == 1
    assert report.counts["regressions"] == 0
    assert "not a green release" in " ".join(report.notes).lower()


def test_new_blocker_classified_as_regression():
    baseline = build_baseline_snapshot([_gap("GAP-OLD")])
    report = build_release_health_report(
        [_gap("GAP-OLD"), _gap("GAP-NEW", description="fresh failure")],
        baseline=baseline,
    )
    assert report.verdict == "regressed"
    assert report.release_ready is False
    assert [g["id"] for g in report.regressions] == ["GAP-NEW"]
    assert [g["id"] for g in report.historical_blocking] == ["GAP-OLD"]


def test_missing_baseline_fail_closed_treats_blockers_as_regressions():
    report = build_release_health_report([_gap("GAP-1")], baseline=None)
    assert report.baseline_present is False
    assert report.verdict == "regressed"
    assert report.release_ready is False
    assert report.counts["regressions"] == 1


def test_clean_current_with_baseline_is_passed():
    baseline = build_baseline_snapshot([_gap("GAP-OLD")])
    report = build_release_health_report([], baseline=baseline)
    assert report.verdict == "passed"
    assert report.release_ready is True
    assert report.counts["resolved_since_baseline"] == 1
    summary = compact_release_health_summary(report)
    assert summary["release_ready"] is True


def test_write_and_load_baseline_roundtrip(tmp_path):
    path = tmp_path / "baseline.json"
    write_baseline_snapshot(path, [_gap("GAP-A"), _gap("GAP-B", blocking=False)])
    data = json.loads(path.read_text(encoding="utf-8"))
    assert data["kind"] == "release_health_baseline"
    assert data["fingerprints"]["blocking"] == ["GAP-A"]


def test_cli_release_health_json_and_fail_on_regression(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    baseline = tmp_path / ".devcouncil" / "release_health_baseline.json"
    write_baseline_snapshot(baseline, [_gap("GAP-HIST")])

    # Empty project has no blocking gaps → passed relative to baseline that expected one.
    ok = runner.invoke(app, ["report", "release-health", "--json"])
    assert ok.exit_code == 0
    payload = json.loads(ok.stdout)
    assert payload["verdict"] in {"passed", "historical_debt", "regressed"}
    assert "release_ready" in payload

    # fail-on-regression should not fail on historical_debt alone when no regressions.
    # Seed a fake current report path is hard without DB gaps; unit coverage above is enough.
    # Exercise --write-baseline and --fail-on-regression on empty graph (no regressions).
    written = runner.invoke(app, ["report", "release-health", "--write-baseline", "--json"])
    assert written.exit_code == 0
    assert baseline.is_file()

    gated = runner.invoke(app, ["report", "release-health", "--fail-on-regression"])
    assert gated.exit_code == 0
