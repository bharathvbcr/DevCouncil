"""`dev map doctor` answers "is anyone else working in this repository?".

The session-start hook warns a session that is starting; the doctor answers the
same question for a session already running, or for a person. The check is
non-critical — a live sibling is not a broken map — but it must not report
"none" when it could not look, and `--fix` must not silently swallow it either.
"""

from __future__ import annotations

from pathlib import Path

import pytest

import devcouncil.devmap_health as health
from devcouncil.utils.git_siblings import DivergentBranch, SessionGuardReport, SiblingCheckout


def _check(result: dict, name: str) -> dict:
    for item in result["checks"]:
        if item["name"] == name:
            return item
    raise AssertionError(f"no {name!r} check in {[c['name'] for c in result['checks']]}")


def _stub(monkeypatch: pytest.MonkeyPatch, report: SessionGuardReport) -> None:
    monkeypatch.setattr(health, "inspect_session_siblings", lambda root, **kw: report)


def test_a_live_sibling_fails_the_check_without_being_critical(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _stub(
        monkeypatch,
        SessionGuardReport(
            live_siblings=(
                SiblingCheckout(
                    path="/repo/.claude/worktrees/other",
                    branch="claude/other",
                    reason="uncommitted changes",
                    age_seconds=120.0,
                ),
            ),
            behind=0,
            default_branch="main",
        ),
    )

    item = _check(health.run_doctor(tmp_path), "siblings")

    assert item["ok"] is False
    assert item["critical"] is False
    assert item["code"] == "live_sibling"
    assert "/repo/.claude/worktrees/other" in item["detail"]
    assert item["fix"]


def test_a_divergent_branch_fails_the_check(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _stub(
        monkeypatch,
        SessionGuardReport(
            divergent_branches=(DivergentBranch(name="claude/x", ahead=3, tip_age_seconds=7200.0),),
            behind=0,
            default_branch="main",
        ),
    )

    item = _check(health.run_doctor(tmp_path), "siblings")

    assert item["ok"] is False
    assert item["code"] == "divergent_branch"
    assert "3 ahead of main" in item["detail"]


def test_a_quiet_repository_passes_with_none(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _stub(monkeypatch, SessionGuardReport(behind=0, default_branch="main"))

    item = _check(health.run_doctor(tmp_path), "siblings")

    assert item["ok"] is True
    assert item["detail"] == "none"


def test_a_probe_that_could_not_run_is_unknown_not_clean(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _stub(
        monkeypatch,
        SessionGuardReport(unavailable=("live sibling checkouts: could not run git: boom",)),
    )

    item = _check(health.run_doctor(tmp_path), "siblings")

    assert item["ok"] is None, "a check that could not run must not report what a passing check reports"
    assert "could not run git" in item["detail"]


def test_being_behind_is_reported_but_does_not_fail(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _stub(monkeypatch, SessionGuardReport(behind=2, default_branch="main"))

    item = _check(health.run_doctor(tmp_path), "siblings")

    assert item["ok"] is True
    assert "2 behind main" in item["detail"]


def test_a_raising_probe_is_reported_not_swallowed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def boom(root, **kwargs):
        raise RuntimeError("git exploded")

    monkeypatch.setattr(health, "inspect_session_siblings", boom)

    item = _check(health.run_doctor(tmp_path), "siblings")

    assert item["ok"] is None
    assert "git exploded" in item["detail"]


def test_the_sibling_codes_are_listed_by_fix_not_dropped() -> None:
    assert "live_sibling" in health._REPORT_ONLY
    assert "divergent_branch" in health._REPORT_ONLY
    # A report-only code must never hold back the build `--fix` would otherwise run.
    assert not health._REPORT_ONLY & health._NEEDS_A_PERSON
