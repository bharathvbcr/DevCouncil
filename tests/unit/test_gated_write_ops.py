"""Unit tests for ``execution/gated_write.py`` (write_file / apply_patch).

DB/lease side effects use a real sqlite state file; git and the ``git apply``
subprocess are mocked so the tests are deterministic and sandbox-safe.
"""

from __future__ import annotations

from pathlib import Path

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution import gated_write as gw
from devcouncil.execution.gated_write import apply_patch_payload, write_file_payload
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.native import FileChangeRepository, TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository

_DIFF = (
    "diff --git a/src/a.py b/src/a.py\n"
    "--- a/src/a.py\n"
    "+++ b/src/a.py\n"
    "@@ -1 +1 @@\n"
    "-x = 1\n"
    "+x = 2\n"
)


def _setup(tmp_path: Path, *, task: Task | None = None, lease: bool = True) -> str | None:
    reset_db_cache()
    dev = tmp_path / ".devcouncil"
    dev.mkdir(exist_ok=True)
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    task = task or Task(
        id="TASK-1", title="t", description="d",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
    )
    token = None
    with db.get_session() as session:
        TaskRepository(session).save(task)
        if lease:
            rec = TaskLeaseRepository(session).acquire("TASK-1", owner="test", ttl_seconds=600)
            token = rec.lease_token
    return token


class _FakeProc:
    def __init__(self, returncode, stderr=""):
        self.returncode = returncode
        self.stderr = stderr
        self.stdout = ""


# --------------------------------------------------------------------------- #
# write_file_payload                                                          #
# --------------------------------------------------------------------------- #

def test_write_not_initialized(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda root: None)
    payload = write_file_payload(tmp_path, task_id="T", lease_token="x", rel_path="a.py", content="y")
    assert payload["code"] == "not_initialized"


def test_write_invalid_lease(tmp_path):
    _setup(tmp_path)
    payload = write_file_payload(tmp_path, task_id="TASK-1", lease_token="bad", rel_path="src/a.py", content="x")
    assert payload["ok"] is False


def test_write_task_not_found(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw.TaskRepository, "get_by_id", lambda self, tid: None)
    payload = write_file_payload(tmp_path, task_id="TASK-1", lease_token=token, rel_path="src/a.py", content="x")
    assert payload["code"] == "not_found"


def test_write_success(tmp_path):
    token = _setup(tmp_path)
    payload = write_file_payload(tmp_path, task_id="TASK-1", lease_token=token, rel_path="src/a.py", content="VALUE = 1\n")
    assert payload["ok"] is True
    assert payload["applied_files"] == ["src/a.py"]
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 1\n"
    reset_db_cache()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        changes = FileChangeRepository(session).list_for_task("TASK-1")
    assert any(c.path == "src/a.py" and c.allowed for c in changes)


def test_write_path_escapes_root(tmp_path):
    token = _setup(tmp_path)
    payload = write_file_payload(tmp_path, task_id="TASK-1", lease_token=token, rel_path="../evil.py", content="x")
    assert payload["ok"] is False
    assert payload["rejected_files"][0]["reason"] == "path escapes the project root"


def test_write_policy_denied_out_of_scope(tmp_path):
    token = _setup(tmp_path)
    payload = write_file_payload(tmp_path, task_id="TASK-1", lease_token=token, rel_path="src/evil.py", content="x")
    assert payload["ok"] is False
    assert payload["rejected_files"]
    assert "src/evil.py" == payload["rejected_files"][0]["path"]


def test_write_os_error(tmp_path, monkeypatch):
    token = _setup(tmp_path)

    def _boom(src, dst):
        raise OSError("disk full")

    monkeypatch.setattr(gw.os, "replace", _boom)
    payload = write_file_payload(tmp_path, task_id="TASK-1", lease_token=token, rel_path="src/a.py", content="x")
    assert payload["ok"] is False
    assert payload["code"] == "write_failed"


# --------------------------------------------------------------------------- #
# apply_patch_payload                                                         #
# --------------------------------------------------------------------------- #

def test_apply_not_initialized(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda root: None)
    payload = apply_patch_payload(tmp_path, task_id="T", lease_token="x", unified_diff=_DIFF)
    assert payload["code"] == "not_initialized"


def test_apply_not_a_git_repo(tmp_path, monkeypatch):
    _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: False)
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token="x", unified_diff=_DIFF)
    assert payload["code"] == "not_a_git_repo"


def test_apply_empty_patch(tmp_path, monkeypatch):
    _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token="x", unified_diff="nothing here")
    assert payload["code"] == "empty_patch"


def test_apply_invalid_lease(tmp_path, monkeypatch):
    _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token="bad", unified_diff=_DIFF)
    assert payload["ok"] is False


def test_apply_task_not_found(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    monkeypatch.setattr(gw.TaskRepository, "get_by_id", lambda self, tid: None)
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=_DIFF)
    assert payload["code"] == "not_found"


def test_apply_rejected_out_of_scope(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    diff = _DIFF.replace("src/a.py", "src/evil.py")
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=diff)
    assert payload["ok"] is False
    assert payload["rejected_files"]


def test_apply_rejected_path_escapes(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    diff = (
        "diff --git a/../evil.py b/../evil.py\n"
        "--- a/../evil.py\n"
        "+++ b/../evil.py\n"
        "@@ -1 +1 @@\n-x\n+y\n"
    )
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=diff)
    assert payload["ok"] is False
    assert any(r["reason"] == "path escapes the project root" for r in payload["rejected_files"])


def test_apply_patch_check_rejected(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    monkeypatch.setattr(gw.subprocess, "run", lambda *a, **k: _FakeProc(1, stderr="does not apply"))
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=_DIFF)
    assert payload["code"] == "patch_rejected"
    # The temp patch file must be cleaned up in the finally block.
    assert not list((tmp_path / ".devcouncil").glob("mcp-apply-*.patch"))


def test_apply_patch_apply_fails(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)

    def _run(cmd, **kwargs):
        # --check passes, the real apply fails.
        return _FakeProc(0) if "--check" in cmd else _FakeProc(1, stderr="apply boom")

    monkeypatch.setattr(gw.subprocess, "run", _run)
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=_DIFF)
    assert payload["code"] == "patch_failed"


def test_apply_unlink_oserror_is_swallowed(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)

    def _run(cmd, **kwargs):
        # Delete the temp patch out from under the finally-block cleanup so
        # patch_path.unlink() raises OSError, which must be swallowed.
        for p in (tmp_path / ".devcouncil").glob("mcp-apply-*.patch"):
            p.unlink()
        return _FakeProc(0)

    monkeypatch.setattr(gw.subprocess, "run", _run)
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=_DIFF)
    assert payload["ok"] is True


def test_apply_success(tmp_path, monkeypatch):
    token = _setup(tmp_path)
    monkeypatch.setattr(gw, "is_git_repo", lambda root: True)
    monkeypatch.setattr(gw.subprocess, "run", lambda *a, **k: _FakeProc(0))
    payload = apply_patch_payload(tmp_path, task_id="TASK-1", lease_token=token, unified_diff=_DIFF)
    assert payload["ok"] is True
    assert payload["applied_files"] == ["src/a.py"]
    reset_db_cache()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        changes = FileChangeRepository(session).list_for_task("TASK-1")
    assert any(c.path == "src/a.py" and c.operation == "apply_patch" and c.allowed for c in changes)
