"""Lease-gated planned-files scope expansion tests."""

from __future__ import annotations

from pathlib import Path

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.policy_engine import TaskPolicyEngine
from devcouncil.execution.task_gate_ops import update_task_scope_payload
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository
from devcouncil.verification.checks.orphan_diff import detect_orphan_diff_gaps
from devcouncil.verification.next_actions import next_action_for
from devcouncil.domain.gap import Gap


def _setup(tmp_path: Path) -> tuple[Path, str, str]:
    reset_db_cache()
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    task = Task(
        id="TASK-1",
        title="t",
        description="d",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        lease = TaskLeaseRepository(session).acquire(
            "TASK-1", owner="test", agent="test", ttl_seconds=600,
        )
        lease_tok = lease.lease_token
    return tmp_path, "TASK-1", lease_tok


def test_planned_file_append_modify_only_under_lease(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup(tmp_path)
    monkeypatch.chdir(root)
    caller = root / "src" / "caller.py"
    caller.parent.mkdir(parents=True, exist_ok=True)
    caller.write_text("x = 1\n", encoding="utf-8")
    payload = update_task_scope_payload(
        root,
        task_id=task_id,
        lease_token=lease_tok,
        planned_files=["src/caller.py"],
    )
    assert payload["ok"] is True
    assert "src/caller.py" in payload["agent_appended_planned_files"]
    pf = next(p for p in payload["planned_files"] if p["path"] == "src/caller.py")
    assert pf["allowed_change"] == "modify"


def test_planned_file_append_rejects_missing_path(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup(tmp_path)
    monkeypatch.chdir(root)
    payload = update_task_scope_payload(
        root,
        task_id=task_id,
        lease_token=lease_tok,
        planned_files=["src/ghost.py"],
    )
    assert payload["ok"] is True
    assert "src/ghost.py" in payload["rejected_planned_files"]
    assert payload["agent_appended_planned_files"] == []


def test_policy_engine_allows_write_after_append(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup(tmp_path)
    monkeypatch.chdir(root)
    caller = root / "src" / "caller.py"
    caller.parent.mkdir(parents=True, exist_ok=True)
    caller.write_text("x = 1\n", encoding="utf-8")
    update_task_scope_payload(
        root, task_id=task_id, lease_token=lease_tok, planned_files=["src/caller.py"],
    )
    reset_db_cache()
    db = Database(root / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
    assert task is not None
    decision = TaskPolicyEngine(root).evaluate_file_change("src/caller.py", task, "write")
    assert decision.action in {"allow", "warn"}


def test_orphan_diff_clears_for_appended_path(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup(tmp_path)
    monkeypatch.chdir(root)
    caller = root / "src" / "caller.py"
    caller.parent.mkdir(parents=True, exist_ok=True)
    caller.write_text("x = 1\n", encoding="utf-8")
    update_task_scope_payload(
        root, task_id=task_id, lease_token=lease_tok, planned_files=["src/caller.py"],
    )
    reset_db_cache()
    db = Database(root / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
    assert task is not None
    planned = {pf.path for pf in task.planned_files}
    gaps = detect_orphan_diff_gaps(
        task=task,
        changed_files=["src/caller.py"],
        planned_paths=planned,
        diff_content="",
        project_root=root,
        get_untracked_files=lambda: [],
        next_gap_id=lambda tid, k: f"{tid}-{k}",
        classify_fn=lambda files: ([], []),
    )
    assert gaps == []


def test_restricted_paths_rejected(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup(tmp_path)
    monkeypatch.chdir(root)
    payload = update_task_scope_payload(
        root,
        task_id=task_id,
        lease_token=lease_tok,
        planned_files=[".env", ".devcouncil/state.sqlite"],
    )
    assert payload["ok"] is True
    assert set(payload["rejected_planned_files"]) == {".env", ".devcouncil/state.sqlite"}
    assert payload["agent_appended_planned_files"] == []


def test_orphan_next_action_points_at_scope_update():
    gap = Gap(
        id="g1",
        severity="high",
        gap_type="orphan_diff",
        description="x",
        recommended_fix="y",
        blocking=True,
        file="src/caller.py",
    )
    action = next_action_for(gap)
    assert "dev scope update" in action.action
    assert "--planned-file" in action.action
