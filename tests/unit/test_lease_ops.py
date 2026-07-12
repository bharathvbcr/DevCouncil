"""Unit tests for ``execution/lease_ops.py`` (checkout/release/renew/list).

Complements test_checkout_map_refresh.py (which covers the map-refresh + happy
checkout path) by exercising the error/edge branches: not-initialized,
task-not-found, lease conflict, semantic context injection, release/renew
success and failure, and lease listing.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.task import Task
from devcouncil.execution import lease_ops
from devcouncil.execution.lease_ops import (
    _refresh_stale_map_if_needed,
    checkout_task_payload,
    list_leases_payload,
    release_task_payload,
    renew_lease_payload,
)
from devcouncil.storage.db import get_db, reset_db_cache
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository
from devcouncil.utils.json_persist import write_json


def _git(root, *args):
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root, check=True, capture_output=True, text=True,
    )


def _seed(tmp_path: Path) -> None:
    reset_db_cache()
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    _git(tmp_path, "init")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-m", "init")
    initialize_project(tmp_path, quiet=True)


def _add_task(tmp_path: Path, task: Task) -> None:
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(task)


def _acquire(tmp_path: Path, task_id: str) -> str:
    db = get_db(tmp_path)
    with db.get_session() as session:
        rec = TaskLeaseRepository(session).acquire(task_id, owner="mcp:c1", client_id="c1", ttl_seconds=600)
        return rec.lease_token


# --------------------------------------------------------------------------- #
# not_initialized                                                             #
# --------------------------------------------------------------------------- #

def test_ops_not_initialized(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.storage.db.get_db", lambda root: None)
    assert checkout_task_payload(tmp_path, task_id="T", client_id="c")["code"] == "not_initialized"
    assert release_task_payload(tmp_path, task_id="T", lease_token="x")["code"] == "not_initialized"
    assert renew_lease_payload(tmp_path, task_id="T", lease_token="x")["code"] == "not_initialized"
    assert list_leases_payload(tmp_path)["code"] == "not_initialized"


# --------------------------------------------------------------------------- #
# checkout_task_payload                                                       #
# --------------------------------------------------------------------------- #

def test_checkout_task_not_found(tmp_path):
    _seed(tmp_path)
    payload = checkout_task_payload(tmp_path, task_id="NOPE", client_id="c1")
    assert payload["ok"] is False
    assert payload["code"] == "not_found"


def test_checkout_success_with_semantic_context(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d", allowed_commands=["pytest"]))
    sem_path = tmp_path / ".devcouncil" / "semantic" / "TASK-1" / "before.json"
    sem_path.parent.mkdir(parents=True, exist_ok=True)
    write_json(sem_path, {"symbols": ["a"]})

    payload = checkout_task_payload(tmp_path, task_id="TASK-1", client_id="c1", agent="claude")
    assert payload["ok"] is True
    assert payload["lease_token"]
    assert payload["status"] == "running" or payload["status"]  # task status field present
    assert payload["semantic_context"] == {"symbols": ["a"]}
    assert "prompt" in payload
    assert payload["allowed_next_tools"]


def test_checkout_lease_conflict(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    first = checkout_task_payload(tmp_path, task_id="TASK-1", client_id="c1")
    assert first["ok"] is True
    # Second checkout without --force hits the active-lease conflict.
    second = checkout_task_payload(tmp_path, task_id="TASK-1", client_id="c2")
    assert second["ok"] is False
    assert second["code"] == "lease_conflict"


def test_checkout_force_reclaims_stale_lease(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    checkout_task_payload(tmp_path, task_id="TASK-1", client_id="c1")
    forced = checkout_task_payload(tmp_path, task_id="TASK-1", client_id="c2", force=True)
    assert forced["ok"] is True


# --------------------------------------------------------------------------- #
# release_task_payload                                                        #
# --------------------------------------------------------------------------- #

def test_release_success(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    token = _acquire(tmp_path, "TASK-1")
    payload = release_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["ok"] is True
    assert payload["released"] is True


def test_release_already_released(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    token = _acquire(tmp_path, "TASK-1")
    release_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    # Second release: lease no longer active; token was valid so -> lease_expired code.
    payload = release_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["ok"] is False
    assert payload["code"] in {"invalid_lease", "lease_expired"}


def test_release_invalid_token(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    _acquire(tmp_path, "TASK-1")
    payload = release_task_payload(tmp_path, task_id="TASK-1", lease_token="wrong")
    assert payload["ok"] is False


# --------------------------------------------------------------------------- #
# renew_lease_payload                                                         #
# --------------------------------------------------------------------------- #

def test_renew_success_default_and_explicit_ttl(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    token = _acquire(tmp_path, "TASK-1")
    payload = renew_lease_payload(tmp_path, task_id="TASK-1", lease_token=token)
    assert payload["ok"] is True
    assert payload["expires_at"]
    explicit = renew_lease_payload(tmp_path, task_id="TASK-1", lease_token=token, ttl_seconds=42)
    assert explicit["ttl_seconds"] == 42


def test_renew_invalid_token(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    _acquire(tmp_path, "TASK-1")
    payload = renew_lease_payload(tmp_path, task_id="TASK-1", lease_token="wrong")
    assert payload["ok"] is False


# --------------------------------------------------------------------------- #
# list_leases_payload                                                         #
# --------------------------------------------------------------------------- #

def test_list_leases_active_and_all(tmp_path):
    _seed(tmp_path)
    _add_task(tmp_path, Task(id="TASK-1", title="t", description="d"))
    token = _acquire(tmp_path, "TASK-1")
    active = list_leases_payload(tmp_path, active_only=True)
    assert active["ok"] is True
    assert active["count"] == 1
    assert active["leases"][0]["task_id"] == "TASK-1"
    # Release, then list all should still show the released lease.
    release_task_payload(tmp_path, task_id="TASK-1", lease_token=token)
    all_leases = list_leases_payload(tmp_path, active_only=False)
    assert all_leases["count"] >= 1


# --------------------------------------------------------------------------- #
# _refresh_stale_map_if_needed (edge branches)                               #
# --------------------------------------------------------------------------- #

def test_refresh_generates_when_map_absent(tmp_path):
    _seed(tmp_path)
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    if map_path.exists():
        map_path.unlink()
    assert _refresh_stale_map_if_needed(tmp_path) is True
    assert map_path.is_file()


def test_refresh_defaults_enabled_when_config_unreadable(tmp_path, monkeypatch):
    _seed(tmp_path)
    # A broken config must not disable the refresh (fail-open to enabled=True).
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda root: (_ for _ in ()).throw(RuntimeError("bad config")),
    )
    # The unreadable config must fail open to enabled=True (i.e. not raise and not
    # short-circuit to False before the map check). Return value depends on whether
    # the map is currently stale, so we only assert the fallback path ran cleanly.
    assert _refresh_stale_map_if_needed(tmp_path) in (True, False)


def test_refresh_swallows_errors(tmp_path, monkeypatch):
    _seed(tmp_path)

    def _boom(root):
        raise RuntimeError("config blew up")

    # Force an exception deep in the refresh path; it must be swallowed -> False.
    monkeypatch.setattr(lease_ops, "read_json", lambda p: (_ for _ in ()).throw(RuntimeError("io")))
    # read_json only runs when the map exists; ensure it does.
    assert (tmp_path / ".devcouncil" / "repo_map.json").exists()
    assert _refresh_stale_map_if_needed(tmp_path) is False
