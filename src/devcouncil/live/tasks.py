from __future__ import annotations

from pathlib import Path

from devcouncil.storage.db import get_db
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository


def active_task_id(project_root: Path) -> str | None:
    """Return the single active DevCouncil task ID, if one is unambiguous.

    Prefers a task in ``running`` status. When none are running, falls back to the
    task behind exactly one non-expired active lease (``dev checkout`` path).
    """
    db = get_db(project_root)
    if not db:
        return None
    with db.get_session() as session:
        running = [task for task in TaskRepository(session).get_all() if task.status == "running"]
        if len(running) == 1:
            return running[0].id
        leases = TaskLeaseRepository(session).list_leases(active_only=True)
        active = [lease for lease, expired in leases if not expired]
        if len(active) == 1:
            return active[0].task_id
        return None
