"""Rank 7 — MCP leases expire, renew, and are listable for fleet supervision."""

import json
from datetime import datetime, timedelta, timezone

import pytest

from devcouncil.domain.task import Task
from devcouncil.integrations.mcp.server import call_tool
from devcouncil.storage.db import Database
from devcouncil.storage.models import TaskLeaseModel
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _db(tmp_path) -> Database:
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    return db


def test_expired_lease_frees_task(tmp_path):
    db = _db(tmp_path)
    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.acquire("TASK-001", owner="a", ttl_seconds=1800)
        # Force its expiry into the past.
        model = session.get(TaskLeaseModel, lease.id)
        model.expires_at = (datetime.now(timezone.utc) - timedelta(seconds=1)).isoformat()
        session.add(model)
        session.commit()
        # active_for_task auto-expires it, so a fresh acquire succeeds without force.
        assert repo.active_for_task("TASK-001") is None
        repo.acquire("TASK-001", owner="b", ttl_seconds=1800)


def test_renew_extends_expiry(tmp_path):
    db = _db(tmp_path)
    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.acquire("TASK-001", owner="a", ttl_seconds=10)
        before = datetime.fromisoformat(lease.expires_at)
        renewed = repo.renew("TASK-001", lease.lease_token, 3600)
        assert renewed is not None
        after = datetime.fromisoformat(renewed.expires_at)
        assert after > before
        # Wrong token does not renew.
        assert repo.renew("TASK-001", "bogus", 3600) is None


def test_list_leases_flags_expired(tmp_path):
    db = _db(tmp_path)
    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.acquire("TASK-001", owner="a", ttl_seconds=1800)
        model = session.get(TaskLeaseModel, lease.id)
        model.expires_at = (datetime.now(timezone.utc) - timedelta(seconds=1)).isoformat()
        session.add(model)
        session.commit()
        pairs = repo.list_leases(active_only=True)
        assert len(pairs) == 1
        _record, expired = pairs[0]
        assert expired is True


@pytest.mark.anyio
async def test_mcp_checkout_sets_expiry_and_renew_list(tmp_path, monkeypatch):
    _db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )
    assert checkout["ok"] is True
    assert checkout["expires_at"]  # TTL is now set (the expiry branch is live)
    token = checkout["lease_token"]

    renewed = json.loads(
        (await call_tool("devcouncil_renew_lease", {"task_id": "TASK-001", "lease_token": token, "ttl_seconds": 60}))[0].text
    )
    assert renewed["ok"] is True
    assert renewed["expires_at"]

    listing = json.loads((await call_tool("devcouncil_list_leases", {}))[0].text)
    assert listing["ok"] is True
    assert listing["count"] == 1
    assert listing["leases"][0]["task_id"] == "TASK-001"
    assert listing["leases"][0]["expired"] is False


def test_partial_unique_index_blocks_second_active_lease(tmp_path):
    # The DB constraint (not just the app-level check) forbids two active leases for a
    # task: a direct second insert is rejected.
    import pytest as _pytest
    from devcouncil.storage.models import TaskLeaseModel
    from sqlalchemy.exc import IntegrityError

    db = _db(tmp_path)
    with db.get_session() as session:
        TaskLeaseRepository(session).acquire("TASK-001", owner="a", ttl_seconds=1800)
    with _pytest.raises(IntegrityError):
        with db.get_session() as session:
            session.add(TaskLeaseModel(
                id="dup", task_id="TASK-001", owner="b", lease_token="t2",
                status="active", created_at="2026-01-01T00:00:00+00:00",
            ))
            session.commit()


def test_dedup_migration_collapses_duplicate_active_leases(tmp_path):
    from sqlalchemy import create_engine, text
    from devcouncil.storage.db import Database

    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db_path = dev / "state.sqlite"
    # Legacy task_leases table WITHOUT the partial unique index, holding two active
    # leases for the same task (a state older DevCouncil versions allowed).
    engine = create_engine(f"sqlite:///{db_path}")
    with engine.begin() as conn:
        conn.execute(text(
            "CREATE TABLE task_leases (id TEXT PRIMARY KEY, task_id TEXT, owner TEXT, "
            "agent TEXT, client_id TEXT, run_id TEXT, branch TEXT, lease_token TEXT, "
            "status TEXT, created_at TEXT, expires_at TEXT, released_at TEXT)"
        ))
        conn.execute(text(
            "INSERT INTO task_leases (id, task_id, owner, lease_token, status, created_at) "
            "VALUES ('old','TASK-001','a','t1','active','2026-01-01T00:00:00+00:00')"
        ))
        conn.execute(text(
            "INSERT INTO task_leases (id, task_id, owner, lease_token, status, created_at) "
            "VALUES ('new','TASK-001','b','t2','active','2026-02-01T00:00:00+00:00')"
        ))
    engine.dispose()

    # Opening the DB migrates: dedup to one active (the newest), then create the index.
    Database(db_path).ensure_schema_version()

    check = create_engine(f"sqlite:///{db_path}")
    with check.begin() as conn:
        active = conn.execute(
            text("SELECT id FROM task_leases WHERE task_id='TASK-001' AND status='active'")
        ).fetchall()
    check.dispose()
    assert [r.id for r in active] == ["new"]  # newest kept, older marked stale
