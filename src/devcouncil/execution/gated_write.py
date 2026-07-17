"""Policy-gated file writes shared by CLI and MCP."""

from __future__ import annotations

import fnmatch
import os
import subprocess
from pathlib import Path

from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.execution.lease_validation import require_valid_lease
from devcouncil.integrations.mcp.util import (
    GIT_APPLY_TIMEOUT_SECONDS,
    diff_target_paths,
    is_git_repo,
    within_root,
)
from devcouncil.storage.native import FileChangeRepository, TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository


def _explicitly_planned(path: str, task) -> bool:  # noqa: ANN001
    normalized = path.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return any(
        normalized == planned.path.replace("\\", "/")
        or fnmatch.fnmatch(normalized, planned.path.replace("\\", "/"))
        for planned in task.planned_files
    )


def write_file_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    rel_path: str,
    content: str,
) -> dict:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        lease_record = TaskLeaseRepository(session).active_for_task(task_id)
        assert lease_record is not None
        task = TaskRepository(session).get_by_id(task_id)
        if not task:
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}

        target = within_root(project_root, rel_path)
        if target is None:
            reason = "path escapes the project root"
            FileChangeRepository(session).record(
                rel_path, "write", False, task_id=task_id, lease_id=lease_record.id, reason=reason,
            )
            return {
                "ok": False, "task_id": task_id, "applied_files": [],
                "rejected_files": [{"path": rel_path, "reason": reason}],
            }

        if not _explicitly_planned(rel_path, task):
            reason = f"Task {task_id} does not explicitly authorize changes to {rel_path}."
            FileChangeRepository(session).record(
                rel_path, "write", False, task_id=task_id, lease_id=lease_record.id, reason=reason,
            )
            return {
                "ok": False, "task_id": task_id, "applied_files": [],
                "rejected_files": [{"path": rel_path, "reason": reason}],
            }

        decision = HookPolicy(project_root=project_root).evaluate_file_write(rel_path, task, content=content)
        if not decision.allowed:
            FileChangeRepository(session).record(
                rel_path, "write", False, task_id=task_id, lease_id=lease_record.id, reason=decision.reason,
            )
            return {
                "ok": False, "task_id": task_id, "applied_files": [],
                "rejected_files": [{"path": rel_path, "reason": decision.reason}],
            }
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            tmp = target.with_name(target.name + ".devcouncil-tmp")
            tmp.write_text(content, encoding="utf-8")
            os.replace(tmp, target)
        except OSError as exc:
            return {"ok": False, "error": f"Write failed: {exc}", "code": "write_failed", "task_id": task_id}
        FileChangeRepository(session).record(
            rel_path, "write", True, task_id=task_id, lease_id=lease_record.id, reason=decision.reason,
        )
        return {
            "ok": True, "task_id": task_id, "applied_files": [rel_path], "rejected_files": [],
        }


def apply_patch_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    unified_diff: str,
) -> dict:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    initialize_project(project_root, quiet=True)
    db = get_db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    if not is_git_repo(project_root):
        return {
            "ok": False,
            "error": "apply_patch requires a git repository. Use write_file instead.",
            "code": "not_a_git_repo",
            "task_id": task_id,
        }
    targets = diff_target_paths(unified_diff)
    if not targets:
        return {"ok": False, "error": "No target files found in the diff.", "code": "empty_patch", "task_id": task_id}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        lease_record = TaskLeaseRepository(session).active_for_task(task_id)
        assert lease_record is not None
        task = TaskRepository(session).get_by_id(task_id)
        if not task:
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}

        policy = HookPolicy(project_root=project_root)
        rejected: list[dict[str, str]] = []
        for path in targets:
            if within_root(project_root, path) is None:
                rejected.append({"path": path, "reason": "path escapes the project root"})
                continue
            if not _explicitly_planned(path, task):
                rejected.append({
                    "path": path,
                    "reason": f"Task {task_id} does not explicitly authorize changes to {path}.",
                })
                continue
            decision = policy.evaluate_file_write(path, task)
            if not decision.allowed:
                rejected.append({"path": path, "reason": decision.reason})
        if rejected:
            for item in rejected:
                FileChangeRepository(session).record(
                    item["path"], "apply_patch", False, task_id=task_id, lease_id=lease_record.id, reason=item["reason"],
                )
            return {
                "ok": False, "task_id": task_id, "applied_files": [], "rejected_files": rejected,
            }

        patch_path = project_root / ".devcouncil" / f"mcp-apply-{lease_record.id}.patch"
        patch_path.parent.mkdir(parents=True, exist_ok=True)
        patch_path.write_text(unified_diff, encoding="utf-8")
        try:
            check = subprocess.run(
                ["git", "apply", "--check", "--ignore-whitespace", str(patch_path)],
                cwd=project_root, capture_output=True, text=True,
                timeout=GIT_APPLY_TIMEOUT_SECONDS,
            )
            if check.returncode != 0:
                return {
                    "ok": False,
                    "error": f"Patch does not apply cleanly: {check.stderr.strip()}",
                    "code": "patch_rejected",
                    "task_id": task_id,
                }
            applied = subprocess.run(
                ["git", "apply", "--ignore-whitespace", str(patch_path)],
                cwd=project_root, capture_output=True, text=True,
                timeout=GIT_APPLY_TIMEOUT_SECONDS,
            )
            if applied.returncode != 0:
                return {
                    "ok": False,
                    "error": f"Patch apply failed: {applied.stderr.strip()}",
                    "code": "patch_failed",
                    "task_id": task_id,
                }
        finally:
            try:
                patch_path.unlink()
            except OSError:
                pass
        for path in targets:
            FileChangeRepository(session).record(
                path, "apply_patch", True, task_id=task_id, lease_id=lease_record.id, reason="policy allowed",
            )
        return {
            "ok": True, "task_id": task_id, "applied_files": targets, "rejected_files": [],
        }
