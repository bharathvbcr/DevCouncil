"""Lease diagnosis for MCP/CLI gates — distinguish expired vs invalid vs conflict."""

from __future__ import annotations

from enum import Enum
from typing import Any

from sqlmodel import Session, col, select

from devcouncil.storage.models import TaskLeaseModel
from devcouncil.storage.native import TaskLeaseRepository


class LeaseCode(str, Enum):
    VALID = "valid"
    LEASE_EXPIRED = "lease_expired"
    INVALID_LEASE = "invalid_lease"
    LEASE_HELD_BY_OTHER = "lease_held_by_other"


def diagnose_lease(session: Session, task_id: str, lease_token: str) -> tuple[LeaseCode, str]:
    """Classify a lease token for actionable agent recovery."""
    repo = TaskLeaseRepository(session)
    active = repo.active_for_task(task_id)
    if active is not None:
        if active.lease_token == lease_token:
            return LeaseCode.VALID, ""
        holder = active.owner or active.client_id or "another agent"
        return (
            LeaseCode.LEASE_HELD_BY_OTHER,
            f"Task {task_id} is held by {holder}. Pick another task or wait for release.",
        )

    stmt = (
        select(TaskLeaseModel)
        .where(TaskLeaseModel.task_id == task_id)
        .where(TaskLeaseModel.lease_token == lease_token)
        .order_by(col(TaskLeaseModel.created_at).desc())
    )
    prior = session.exec(stmt).first()
    if prior is not None and prior.status in {"stale", "released"}:
        return (
            LeaseCode.LEASE_EXPIRED,
            "Lease TTL expired. Check out the task again with devcouncil_checkout_task.",
        )
    return LeaseCode.INVALID_LEASE, "Invalid lease token."


def lease_error_payload(code: LeaseCode, message: str, task_id: str) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "ok": False,
        "error": message,
        "code": code.value,
        "task_id": task_id,
    }
    if code is LeaseCode.LEASE_EXPIRED:
        payload["suggested_action"] = "checkout_again"
        payload["suggested_tool"] = "devcouncil_checkout_task"
        payload["hint"] = "Renew only works before TTL expiry; call checkout again after expiry."
    elif code is LeaseCode.LEASE_HELD_BY_OTHER:
        payload["suggested_action"] = "pick_other_task"
        payload["suggested_tool"] = "devcouncil_next_task"
    return payload


def require_valid_lease(session: Session, task_id: str, lease_token: str) -> dict[str, Any] | None:
    """Return an error payload when the lease is not valid, else None."""
    code, message = diagnose_lease(session, task_id, lease_token)
    if code is LeaseCode.VALID:
        return None
    return lease_error_payload(code, message, task_id)
