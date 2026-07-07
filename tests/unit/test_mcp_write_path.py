"""Rank 4 — lease-gated MCP write path (write_file + apply_patch).

A pure-MCP agent can now make the actual code change through a channel DevCouncil
gates (out-of-scope paths rejected) and records (FileChangeEvent provenance), so the
loop is closable over MCP alone.
"""

import json
import os
import subprocess

import pytest

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.integrations.mcp.server import call_tool
from devcouncil.storage.db import Database
from devcouncil.storage.models import FileChangeEventModel
from devcouncil.storage.repositories import TaskRepository
from sqlmodel import select


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _setup(tmp_path, planned=("src/a.py",)):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path=p, reason="x", allowed_change="modify") for p in planned],
        ))
    return db


async def _checkout(task_id="TASK-001"):
    res = json.loads((await call_tool("devcouncil_checkout_task", {"task_id": task_id, "client_id": "a"}))[0].text)
    return res["lease_token"]


def _file_events(db):
    with db.get_session() as session:
        return [(e.path, e.allowed) for e in session.exec(select(FileChangeEventModel)).all()]


@pytest.mark.anyio
async def test_write_file_allows_planned_and_records(tmp_path, monkeypatch):
    db = _setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    token = await _checkout()

    res = json.loads((await call_tool("devcouncil_write_file", {
        "task_id": "TASK-001", "lease_token": token, "path": "src/a.py", "content": "VALUE = 1\n",
    }))[0].text)

    assert res["ok"] is True
    assert res["applied_files"] == ["src/a.py"]
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 1\n"
    assert any(path == "src/a.py" and allowed for path, allowed in _file_events(db))


@pytest.mark.anyio
async def test_write_file_rejects_unplanned_and_does_not_write(tmp_path, monkeypatch):
    db = _setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    token = await _checkout()

    res = json.loads((await call_tool("devcouncil_write_file", {
        "task_id": "TASK-001", "lease_token": token, "path": "src/evil.py", "content": "x = 1\n",
    }))[0].text)

    assert res["ok"] is False
    assert res["applied_files"] == []
    assert res["rejected_files"][0]["path"] == "src/evil.py"
    assert not (tmp_path / "src" / "evil.py").exists()
    assert any(path == "src/evil.py" and not allowed for path, allowed in _file_events(db))


@pytest.mark.anyio
async def test_write_file_rejects_path_escape(tmp_path, monkeypatch):
    _setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    token = await _checkout()

    res = json.loads((await call_tool("devcouncil_write_file", {
        "task_id": "TASK-001", "lease_token": token, "path": "../escape.py", "content": "x=1\n",
    }))[0].text)
    assert res["ok"] is False
    assert "escapes" in res["rejected_files"][0]["reason"]


@pytest.mark.anyio
async def test_write_file_rejects_bad_lease(tmp_path, monkeypatch):
    _setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    await _checkout()
    res = json.loads((await call_tool("devcouncil_write_file", {
        "task_id": "TASK-001", "lease_token": "wrong", "path": "src/a.py", "content": "x=1\n",
    }))[0].text)
    assert res["ok"] is False
    assert res["code"] == "invalid_lease"


def _git(root, *args):
    if args and args[0] == "init":
        env = {
            **os.environ,
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "init.templateDir",
            "GIT_CONFIG_VALUE_0": "/dev/null",
        }
        subprocess.run(
            ["git", "init", "-b", "main"],
            cwd=root, check=True, capture_output=True, text=True, env=env,
        )
        return
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


@pytest.mark.anyio
async def test_apply_patch_applies_planned(tmp_path, monkeypatch):
    db = _setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("VALUE = 1\n", encoding="utf-8")
    _git(tmp_path, "init")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")
    # Build a real diff: modify, capture, restore.
    (tmp_path / "src" / "a.py").write_text("VALUE = 2\n", encoding="utf-8")
    diff = subprocess.run(["git", "diff"], cwd=tmp_path, capture_output=True, text=True).stdout
    _git(tmp_path, "checkout", "--", "src/a.py")

    token = await _checkout()
    res = json.loads((await call_tool("devcouncil_apply_patch", {
        "task_id": "TASK-001", "lease_token": token, "unified_diff": diff,
    }))[0].text)

    assert res["ok"] is True
    assert res["applied_files"] == ["src/a.py"]
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 2\n"
    assert any(path == "src/a.py" and allowed for path, allowed in _file_events(db))


@pytest.mark.anyio
async def test_apply_patch_rejects_whole_patch_if_any_target_unplanned(tmp_path, monkeypatch):
    # Only src/a.py is planned; the patch also touches src/evil.py -> whole patch rejected.
    _setup(tmp_path, planned=("src/a.py",))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "src" / "evil.py").write_text("E = 1\n", encoding="utf-8")
    _git(tmp_path, "init")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")
    (tmp_path / "src" / "a.py").write_text("VALUE = 2\n", encoding="utf-8")
    (tmp_path / "src" / "evil.py").write_text("E = 2\n", encoding="utf-8")
    diff = subprocess.run(["git", "diff"], cwd=tmp_path, capture_output=True, text=True).stdout
    _git(tmp_path, "checkout", "--", "src/a.py", "src/evil.py")

    token = await _checkout()
    res = json.loads((await call_tool("devcouncil_apply_patch", {
        "task_id": "TASK-001", "lease_token": token, "unified_diff": diff,
    }))[0].text)

    assert res["ok"] is False
    assert {r["path"] for r in res["rejected_files"]} == {"src/evil.py"}
    # Nothing applied — the planned file is untouched too.
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 1\n"
