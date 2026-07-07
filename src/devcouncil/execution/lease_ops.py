"""Task lease operations shared by CLI and MCP."""

from __future__ import annotations

from pathlib import Path

from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.integrations.mcp.util import allowed_next_tools, lease_ttl_seconds
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import GapRepository, RequirementRepository, TaskRepository
from devcouncil.utils.json_persist import read_json


def checkout_task_payload(
    project_root: Path,
    *,
    task_id: str,
    client_id: str,
    agent: str | None = None,
    force: bool = False,
) -> dict:
    """Acquire a task lease and return scope for gated write tools."""
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        task = task_repo.get_by_id(task_id)
        if not task:
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}
        lease_repo = TaskLeaseRepository(session)
        try:
            lease = lease_repo.acquire(
                task_id,
                owner=f"mcp:{client_id}",
                agent=agent,
                client_id=client_id,
                ttl_seconds=lease_ttl_seconds(project_root),
                force=force,
            )
        except ValueError as exc:
            return {"ok": False, "error": str(exc), "code": "lease_conflict", "task_id": task_id}
        prompt = PromptBuilder(project_root).build_task_prompt(task, RequirementRepository(session).get_all())
        semantic = None
        semantic_path = project_root / ".devcouncil" / "semantic" / task_id / "before.json"
        if semantic_path.exists():
            semantic = read_json(semantic_path)
        return {
            "ok": True,
            "lease_token": lease.lease_token,
            "task_id": task.id,
            "status": task.status,
            "expires_at": lease.expires_at,
            "prompt": prompt,
            "planned_files": [f.model_dump() for f in task.planned_files],
            "allowed_commands": task.allowed_commands,
            "expected_tests": task.expected_tests,
            "semantic_context": semantic,
            "allowed_next_tools": allowed_next_tools(
                "running",
                bool(GapRepository(session).get_blocking_for_task(task_id)),
            ),
        }


def release_task_payload(project_root: Path, *, task_id: str, lease_token: str) -> dict:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        released = TaskLeaseRepository(session).release(task_id, lease_token)
        if not released:
            return {"ok": False, "error": "Invalid lease token.", "code": "invalid_lease", "task_id": task_id}
        return {"ok": True, "task_id": task_id, "released": True}


def renew_lease_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    ttl_seconds: int | None = None,
) -> dict:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    ttl = ttl_seconds if ttl_seconds is not None else lease_ttl_seconds(project_root)
    with db.get_session() as session:
        renewed_lease = TaskLeaseRepository(session).renew(task_id, lease_token, ttl)
        if renewed_lease is None:
            return {"ok": False, "error": "Invalid or expired lease.", "code": "invalid_lease", "task_id": task_id}
        return {
            "ok": True,
            "task_id": task_id,
            "expires_at": renewed_lease.expires_at,
            "ttl_seconds": ttl,
        }


def list_leases_payload(project_root: Path, *, active_only: bool = True) -> dict:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        pairs = TaskLeaseRepository(session).list_leases(active_only=active_only)
    leases = [
        {
            "task_id": lease.task_id,
            "owner": lease.owner,
            "agent": lease.agent,
            "status": lease.status,
            "expires_at": lease.expires_at,
            "expired": expired,
        }
        for lease, expired in pairs
    ]
    return {"ok": True, "leases": leases, "count": len(leases)}
