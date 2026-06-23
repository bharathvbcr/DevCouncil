"""Rank 5 — the repair contract is persisted and resumable.

Gap routing fields (file/line/suggested_command/acceptance_criterion_id) survive a
reload, the v4 column migration adds them to a pre-existing DB, NextAction exposes
ac_id, and the MCP read tools return outstanding work without re-verifying.
"""

import json

import pytest
from sqlalchemy import create_engine, text

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.integrations.mcp.server import call_tool
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository
from devcouncil.verification.next_actions import build_next_actions


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _gap() -> Gap:
    return Gap(
        id="GAP-1", severity="high", gap_type="acceptance_criteria_unproven",
        task_id="TASK-001", description="AC-001 unproven", recommended_fix="add a test",
        blocking=True, file="src/auth.py", line=42,
        suggested_command="pytest tests/test_auth.py", acceptance_criterion_id="AC-001",
    )


def test_gap_routing_fields_round_trip(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        GapRepository(session).save(_gap())
    with db.get_session() as session:
        reloaded = GapRepository(session).get_all()[0]
    assert reloaded.file == "src/auth.py"
    assert reloaded.line == 42
    assert reloaded.suggested_command == "pytest tests/test_auth.py"
    assert reloaded.acceptance_criterion_id == "AC-001"


def test_next_action_exposes_acceptance_criterion_id():
    actions = build_next_actions([_gap()])
    assert actions[0].acceptance_criterion_id == "AC-001"
    assert actions[0].file == "src/auth.py"
    assert actions[0].line == 42


def test_column_migration_adds_gap_fields_to_legacy_db(tmp_path):
    # Simulate a pre-v4 database whose gaps table lacks the routing columns.
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db_path = dev / "state.sqlite"
    engine = create_engine(f"sqlite:///{db_path}")
    with engine.begin() as conn:
        conn.execute(text(
            "CREATE TABLE gaps (id TEXT PRIMARY KEY, severity TEXT, gap_type TEXT, "
            "requirement_id TEXT, task_id TEXT, description TEXT, evidence_json TEXT, "
            "recommended_fix TEXT, blocking BOOLEAN)"
        ))
        conn.execute(text(
            "INSERT INTO gaps VALUES ('GAP-OLD','high','test_failed',NULL,'TASK-001',"
            "'old gap','[]','fix it',1)"
        ))
    engine.dispose()

    # Opening the DB must migrate the schema, then reads must succeed and new saves
    # round-trip the routing fields.
    db = Database(db_path)
    db.ensure_schema_version()
    with db.get_session() as session:
        existing = GapRepository(session).get_all()
        assert existing[0].id == "GAP-OLD"
        assert existing[0].file is None
        GapRepository(session).save(_gap())
    with db.get_session() as session:
        new = [g for g in GapRepository(session).get_all() if g.id == "GAP-1"][0]
        assert new.acceptance_criterion_id == "AC-001"


@pytest.mark.anyio
async def test_mcp_get_gaps_and_next_actions_without_reverify(tmp_path, monkeypatch):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
        GapRepository(session).save(_gap())
        GapRepository(session).save(Gap(
            id="GAP-2", severity="medium", gap_type="diff_not_exercised", task_id="TASK-001",
            description="changed lines not run", recommended_fix="add test", blocking=False,
        ))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    gaps_payload = json.loads((await call_tool("devcouncil_get_gaps", {"task_id": "TASK-001"}))[0].text)
    assert gaps_payload["ok"] is True
    assert gaps_payload["blocking_count"] == 1
    assert {g["id"] for g in gaps_payload["gaps"]} == {"GAP-1", "GAP-2"}

    na_payload = json.loads((await call_tool("devcouncil_get_next_actions", {"task_id": "TASK-001"}))[0].text)
    assert [a["gap_id"] for a in na_payload["next_actions"]] == ["GAP-1"]
    assert [a["gap_id"] for a in na_payload["advisory_actions"]] == ["GAP-2"]
    assert na_payload["next_actions"][0]["acceptance_criterion_id"] == "AC-001"
    assert "devcouncil_verify_task" in na_payload["allowed_next_tools"]
