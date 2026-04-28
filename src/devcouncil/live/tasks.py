from __future__ import annotations

from pathlib import Path

from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository


def active_task_id(project_root: Path) -> str | None:
    """Return the single running DevCouncil task ID, if one is unambiguous."""
    db = get_db(project_root)
    if not db:
        return None
    with db.get_session() as session:
        running = [task for task in TaskRepository(session).get_all() if task.status == "running"]
    if len(running) != 1:
        return None
    return running[0].id
