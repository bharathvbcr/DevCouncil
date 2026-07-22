"""F-15 contract: CLI write scope and MCP read/diff scope must not drift.

Observable bug before the shared helper: MCP ``read_file`` / ``get_diff`` used
exact path set membership while CLI gated writes used ``fnmatch`` + ``./``
stripping. A glob planned file like ``pkg/*.py`` allowed writes but denied MCP
reads of ``pkg/a.py``.
"""

from __future__ import annotations

import json
import subprocess

import pytest

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.gated_write import _explicitly_planned
from devcouncil.execution.planned_scope import matches_planned_path
from devcouncil.integrations.mcp.server import call_tool
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _init_db(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    return db


def _git_init(root):
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "t@t.com"],
        ["git", "config", "user.name", "t"],
    ):
        subprocess.run(args, cwd=root, capture_output=True, text=True)


def test_matches_planned_path_agrees_with_gated_write_helper():
    task = Task(
        id="TASK-1",
        title="t",
        description="d",
        planned_files=[PlannedFile(path="pkg/*.py", reason="r", allowed_change="modify")],
    )
    assert matches_planned_path("pkg/a.py", task.planned_files) is True
    assert _explicitly_planned("pkg/a.py", task) is True
    assert matches_planned_path("./pkg/a.py", task.planned_files) is True
    assert _explicitly_planned("./pkg/a.py", task) is True
    assert matches_planned_path("other/a.py", task.planned_files) is False
    assert _explicitly_planned("other/a.py", task) is False


@pytest.mark.anyio
async def test_mcp_read_honors_glob_planned_like_write_policy(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "a.py").write_text("ok\n", encoding="utf-8")
    (pkg / "b.txt").write_text("nope\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                planned_files=[
                    PlannedFile(path="pkg/*.py", reason="r", allowed_change="modify"),
                ],
            )
        )

    allowed = json.loads(
        (await call_tool("devcouncil_read_file", {"path": "pkg/a.py", "task_id": "TASK-001"}))[0].text
    )
    assert allowed["ok"] is True
    assert allowed["content"] == "ok"

    denied = json.loads(
        (await call_tool("devcouncil_read_file", {"path": "pkg/b.txt", "task_id": "TASK-001"}))[0].text
    )
    assert denied["ok"] is False
    assert denied["code"] == "out_of_scope"


@pytest.mark.anyio
async def test_mcp_read_accepts_dot_slash_prefix_like_write_policy(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "a.py").write_text("ok\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
            )
        )

    payload = json.loads(
        (await call_tool("devcouncil_read_file", {"path": "./a.py", "task_id": "TASK-001"}))[0].text
    )
    assert payload["ok"] is True


@pytest.mark.anyio
async def test_mcp_get_diff_intersects_explicit_paths_with_glob_planned(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "a.py").write_text("a=1\n", encoding="utf-8")
    (pkg / "b.py").write_text("b=1\n", encoding="utf-8")
    (tmp_path / "c.py").write_text("c=1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (pkg / "a.py").write_text("a=1\na2=2\n", encoding="utf-8")
    (pkg / "b.py").write_text("b=1\nb2=2\n", encoding="utf-8")
    (tmp_path / "c.py").write_text("c=1\nc2=2\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                planned_files=[
                    PlannedFile(path="pkg/*.py", reason="r", allowed_change="modify"),
                ],
            )
        )

    payload = json.loads(
        (
            await call_tool(
                "devcouncil_get_diff",
                {"task_id": "TASK-001", "paths": ["pkg/a.py", "pkg/b.py", "c.py"]},
            )
        )[0].text
    )
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "pkg/a.py" in paths
    assert "pkg/b.py" in paths
    assert "c.py" not in paths
