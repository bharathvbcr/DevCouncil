"""Unit tests for lease diagnosis codes."""

from datetime import datetime, timedelta, timezone

from devcouncil.domain.task import Task
from devcouncil.execution.lease_validation import LeaseCode, diagnose_lease, require_valid_lease
from devcouncil.storage.db import Database
from devcouncil.storage.models import TaskLeaseModel
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository


def _db(tmp_path) -> Database:
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    return db


def test_expired_lease_returns_lease_expired_code(tmp_path):
    db = _db(tmp_path)
    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.acquire("TASK-001", owner="a", ttl_seconds=60)
        model = session.get(TaskLeaseModel, lease.id)
        model.expires_at = (datetime.now(timezone.utc) - timedelta(seconds=1)).isoformat()
        session.add(model)
        session.commit()
        code, message = diagnose_lease(session, "TASK-001", lease.lease_token)
        assert code is LeaseCode.LEASE_EXPIRED
        assert "renew_lease" not in message.lower()
        assert "devcouncil_checkout_task" in message
        payload = require_valid_lease(session, "TASK-001", lease.lease_token)
        assert payload is not None
        assert payload["code"] == "lease_expired"
        assert payload["suggested_tool"] == "devcouncil_checkout_task"


def test_other_holder_returns_lease_held_by_other(tmp_path):
    db = _db(tmp_path)
    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.acquire("TASK-001", owner="agent-a", ttl_seconds=60)
        code, message = diagnose_lease(session, "TASK-001", "wrong-token")
        assert code is LeaseCode.LEASE_HELD_BY_OTHER
        assert "agent-a" in message
        assert lease.lease_token != "wrong-token"
