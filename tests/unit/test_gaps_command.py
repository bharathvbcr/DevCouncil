import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import GapRepository

runner = CliRunner()


def test_gaps_command_lists_blocking_and_advisory(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        GapRepository(session).save(
            Gap(
                id="GAP-BLOCK-1",
                severity="high",
                gap_type="orphan_diff",
                task_id="TASK-001",
                description="Blocking orphan diff",
                recommended_fix="Revert",
                blocking=True,
            )
        )
        GapRepository(session).save(
            Gap(
                id="GAP-ADV-1",
                severity="medium",
                gap_type="stub_detected",
                task_id="TASK-001",
                description="Advisory stub",
                recommended_fix="Remove stub",
                blocking=False,
            )
        )

    result = runner.invoke(app, ["gaps", "--json"])
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["total_count"] == 2
    assert payload["blocking_count"] == 1
    assert payload["advisory_count"] == 1
    ids = {gap["id"] for gap in payload["gaps"]}
    assert ids == {"GAP-BLOCK-1", "GAP-ADV-1"}


def test_gaps_blocking_only_and_fail_on_blocking(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        GapRepository(session).save(
            Gap(
                id="GAP-BLOCK-2",
                severity="critical",
                gap_type="test_failed",
                task_id="TASK-002",
                description="Tests failed",
                recommended_fix="Fix tests",
                blocking=True,
            )
        )

    filtered = runner.invoke(app, ["gaps", "--json", "--blocking-only"])
    assert filtered.exit_code == 0
    data = json.loads(filtered.stdout)
    assert data["total_count"] == 1
    assert data["gaps"][0]["id"] == "GAP-BLOCK-2"

    fail = runner.invoke(app, ["gaps", "--fail-on-blocking"])
    assert fail.exit_code == 1
