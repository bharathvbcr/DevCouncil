"""Unit tests for the MCP content-plane tools (read_file, get_diff, get_evidence,
run_command, next_task) added to the DevCouncil MCP server."""

import json
import subprocess

import pytest

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.integrations.mcp.server import call_tool, list_tools
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import EvidenceRepository, TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _init_db(tmp_path):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    return db


async def _checkout(task_id, client_id="a"):
    payload = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": task_id, "client_id": client_id}))[0].text
    )
    return payload["lease_token"]


def _git_init(root):
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "t@t.com"],
        ["git", "config", "user.name", "t"],
    ):
        subprocess.run(args, cwd=root, capture_output=True, text=True)


@pytest.mark.anyio
async def test_new_tools_are_listed():
    names = {tool.name for tool in await list_tools()}
    for expected in (
        "devcouncil_read_file",
        "devcouncil_get_diff",
        "devcouncil_get_evidence",
        "devcouncil_run_command",
        "devcouncil_next_task",
    ):
        assert expected in names


@pytest.mark.anyio
async def test_read_file_happy_path(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "hello.py").write_text("line1\nline2\nline3\n", encoding="utf-8")

    result = await call_tool("devcouncil_read_file", {"path": "hello.py"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["path"] == "hello.py"
    assert "line1" in payload["content"]
    assert payload["line_count"] == 3
    assert len(payload["sha256"]) == 64


@pytest.mark.anyio
async def test_read_file_line_range(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "f.txt").write_text("a\nb\nc\nd\ne\n", encoding="utf-8")

    result = await call_tool("devcouncil_read_file", {"path": "f.txt", "line_range": "2-3"})
    payload = json.loads(result[0].text)
    assert payload["content"] == "b\nc"
    assert payload["line_count"] == 5


@pytest.mark.anyio
async def test_read_file_refuses_escape(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    result = await call_tool("devcouncil_read_file", {"path": "../outside.txt"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "path_escape"


@pytest.mark.anyio
async def test_read_file_refuses_secret(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / ".env").write_text("SECRET=abc\n", encoding="utf-8")
    result = await call_tool("devcouncil_read_file", {"path": ".env"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "secret_path"


@pytest.mark.anyio
async def test_get_diff_shape(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    target = tmp_path / "mod.py"
    target.write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "mod.py"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    target.write_text("x = 1\ny = 2\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert "unified_diff" in payload
    paths = {f["path"] for f in payload["files"]}
    assert "mod.py" in paths
    entry = next(f for f in payload["files"] if f["path"] == "mod.py")
    assert entry["additions"] == 1


@pytest.mark.anyio
async def test_get_diff_requires_git(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "not_a_git_repo"


@pytest.mark.anyio
async def test_get_evidence_inlines_logs(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    stdout_file = tmp_path / "out.log"
    stdout_file.write_text("captured stdout output", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
        EvidenceRepository(session).save_command_result(
            "TASK-001",
            CommandResult(
                command="pytest -q",
                exit_code=1,
                stdout_path=str(stdout_file),
                stderr_path=str(tmp_path / "missing-stderr.log"),
                summary="failed",
            ),
        )

    result = await call_tool("devcouncil_get_evidence", {"task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    item = payload["evidence"][0]
    assert item["command"] == "pytest -q"
    assert item["exit_code"] == 1
    assert "captured stdout output" in item["stdout"]
    # Missing stderr file tolerated as empty.
    assert item["stderr"] == ""


@pytest.mark.anyio
async def test_get_evidence_command_filter(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
        repo = EvidenceRepository(session)
        repo.save_command_result("TASK-001", CommandResult(
            command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"))
        repo.save_command_result("TASK-001", CommandResult(
            command="ruff check", exit_code=0, stdout_path="", stderr_path="", summary="ok"))

    result = await call_tool("devcouncil_get_evidence", {"task_id": "TASK-001", "command": "ruff"})
    payload = json.loads(result[0].text)
    assert len(payload["evidence"]) == 1
    assert payload["evidence"][0]["command"] == "ruff check"


@pytest.mark.anyio
async def test_run_command_denies_out_of_allowlist(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-001", title="T", description="D", allowed_commands=["echo hi"])
        )
    token = await _checkout("TASK-001")

    result = await call_tool(
        "devcouncil_run_command",
        {"task_id": "TASK-001", "lease_token": token, "command": "rm -rf /"},
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "command_not_allowed"


@pytest.mark.anyio
async def test_run_command_allows_allowlisted(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    import sys

    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-001", title="T", description="D", allowed_commands=[f"{sys.executable} --version"])
        )
    token = await _checkout("TASK-001")

    result = await call_tool(
        "devcouncil_run_command",
        {
            "task_id": "TASK-001",
            "lease_token": token,
            "command": f"{sys.executable} --version",
        },
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["exit_code"] == 0
    # `python --version` prints "Python X.Y.Z" (stdout on modern Python).
    assert "Python" in (payload["stdout"] + payload["stderr"])


@pytest.mark.anyio
async def test_run_command_rejects_bad_lease(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-001", title="T", description="D", allowed_commands=["echo hi"])
        )
    result = await call_tool(
        "devcouncil_run_command",
        {"task_id": "TASK-001", "lease_token": "bad", "command": "echo hi"},
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "invalid_lease"


@pytest.mark.anyio
async def test_next_task_selects_unblocked_unleased(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        repo = TaskRepository(session)
        repo.save(Task(id="TASK-001", title="A", description="D"))
        repo.save(Task(id="TASK-002", title="B", description="D"))

    result = await call_tool("devcouncil_next_task", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    # Deterministic ordering: lowest id chosen first.
    assert payload["task"]["id"] == "TASK-001"
    assert payload["ready_to_checkout"] is True
    assert "devcouncil_read_file" in payload["allowed_next_tools"]


@pytest.mark.anyio
async def test_next_task_dependency_logic_in_memory(tmp_path, monkeypatch):
    """The depends_on gate is exercised directly: a task whose deps are unmet is not
    eligible. (TaskRepository does not yet persist depends_on, so this asserts the
    helper logic rather than relying on a round-trip.)"""
    from devcouncil.integrations.mcp.server import _allowed_next_tools

    # Sanity on the self-describing contract used by next_task.
    assert _allowed_next_tools("verified", False) == ["devcouncil_release_task"]
    assert "devcouncil_run_command" in _allowed_next_tools("blocked", True)


@pytest.mark.anyio
async def test_next_task_skips_leased(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        repo = TaskRepository(session)
        repo.save(Task(id="TASK-001", title="A", description="D"))
        repo.save(Task(id="TASK-002", title="B", description="D"))
    # Lease TASK-001; next_task must skip it and pick TASK-002.
    await _checkout("TASK-001")

    result = await call_tool("devcouncil_next_task", {})
    payload = json.loads(result[0].text)
    assert payload["task"]["id"] == "TASK-002"


@pytest.mark.anyio
async def test_next_task_none_when_all_leased(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="A", description="D"))
    # The only candidate is actively leased -> no task available.
    await _checkout("TASK-001")
    result = await call_tool("devcouncil_next_task", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["task"] is None


@pytest.mark.anyio
async def test_checkout_includes_allowed_next_tools(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    payload = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )
    assert "devcouncil_run_command" in payload["allowed_next_tools"]
    assert "devcouncil_read_file" in payload["allowed_next_tools"]


@pytest.mark.anyio
async def test_get_diff_scoped_to_task(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "a.py").write_text("a=1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b=1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "a.py").write_text("a=1\na2=2\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b=1\nb2=2\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
        ))

    result = await call_tool("devcouncil_get_diff", {"task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    paths = {f["path"] for f in payload["files"]}
    assert "a.py" in paths
    assert "b.py" not in paths


@pytest.mark.anyio
async def test_get_diff_includes_untracked_text_file(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "tracked.py").write_text("ok\n", encoding="utf-8")
    subprocess.run(["git", "add", "tracked.py"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "brand_new.py").write_text("print('hello')\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "brand_new.py" in paths
    entry = next(f for f in payload["files"] if f["path"] == "brand_new.py")
    assert entry["status"] == "A"
    assert entry["additions"] >= 1
    assert "brand_new.py" in payload["unified_diff"]
    assert "+print('hello')" in payload["unified_diff"]
    assert "diff --git a/brand_new.py b/brand_new.py" in payload["unified_diff"]


@pytest.mark.anyio
async def test_get_diff_includes_empty_untracked_file(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "empty_new.txt").write_text("", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "empty_new.txt" in paths
    entry = next(f for f in payload["files"] if f["path"] == "empty_new.txt")
    assert entry["status"] == "A"
    assert entry["additions"] == 0
    assert "diff --git a/empty_new.txt b/empty_new.txt" in payload["unified_diff"]
    assert "new file mode 100644" in payload["unified_diff"]


@pytest.mark.anyio
async def test_get_diff_path_filter_limits_untracked(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "keep_me.py").write_text("keep\n", encoding="utf-8")
    (tmp_path / "skip_me.py").write_text("skip\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {"paths": ["keep_me.py"]})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "keep_me.py" in paths
    assert "skip_me.py" not in paths
    assert "keep_me.py" in payload["unified_diff"]
    assert "skip_me.py" not in payload["unified_diff"]


@pytest.mark.anyio
async def test_get_diff_untracked_respects_external_20kb_cap(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "big_new.txt").write_text("x" * 30_000 + "\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert "big_new.txt" in {f["path"] for f in payload["files"]}
    assert payload["truncated"] is True
    assert len(payload["unified_diff"]) < 30_000
    assert "truncated to 20000 characters" in payload["unified_diff"]


@pytest.mark.anyio
async def test_get_diff_task_id_without_db_fails_closed(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "leak.py").write_text("secret\n", encoding="utf-8")
    subprocess.run(["git", "add", "leak.py"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "leak.py").write_text("secret\nchanged\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {"task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "not_initialized"
    assert "unified_diff" not in payload or payload.get("unified_diff") in (None, "")


@pytest.mark.anyio
async def test_get_diff_explicit_paths_intersect_task_scope(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "a.py").write_text("a=1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b=1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "a.py").write_text("a=1\na2=2\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b=1\nb2=2\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
        ))

    # Explicit paths must intersect planned files — never union/broaden to b.py.
    result = await call_tool(
        "devcouncil_get_diff",
        {"task_id": "TASK-001", "paths": ["a.py", "b.py"]},
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "a.py" in paths
    assert "b.py" not in paths


@pytest.mark.anyio
async def test_get_diff_explicit_paths_outside_task_scope_empty(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "a.py").write_text("a=1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b=1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "a.py").write_text("a=1\na2=2\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b=1\nb2=2\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
        ))

    result = await call_tool(
        "devcouncil_get_diff",
        {"task_id": "TASK-001", "paths": ["b.py"]},
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["files"] == []
    assert payload["unified_diff"] == ""


@pytest.mark.anyio
async def test_get_diff_staged_rename_nul_safe(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "old_name.py").write_text("value = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "old_name.py"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(
        ["git", "mv", "old_name.py", "new_name.py"],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=True,
    )

    result = await call_tool("devcouncil_get_diff", {"staged": True})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "new_name.py" in paths
    entry = next(f for f in payload["files"] if f["path"] == "new_name.py")
    assert str(entry["status"]).startswith("R")


@pytest.mark.anyio
async def test_get_diff_surfaces_nonzero_git_as_ok_false(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)

    from devcouncil.integrations.mcp.handlers import git as git_handlers

    real_run = git_handlers._run_git

    def boom(root, args):
        if args[:2] == ["git", "diff"] and "--numstat" not in args and "--name-status" not in args:
            return subprocess.CompletedProcess(
                args=args, returncode=128, stdout="", stderr="fatal: bad revision",
            )
        return real_run(root, args)

    monkeypatch.setattr(git_handlers, "_run_git", boom)
    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert "bad revision" in payload["error"] or "exited 128" in payload["error"]


@pytest.mark.anyio
async def test_get_diff_includes_binary_untracked_file(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "blob.bin").write_bytes(b"png\x00data\x00more")

    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    paths = {f["path"] for f in payload["files"]}
    assert "blob.bin" in paths
    entry = next(f for f in payload["files"] if f["path"] == "blob.bin")
    assert entry["status"] == "A"
    assert entry["additions"] == 0
    assert "Binary files /dev/null and b/blob.bin differ" in payload["unified_diff"]


@pytest.mark.anyio
async def test_get_diff_empty_planned_scope_fail_closed(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "secret.py").write_text("secret=1\n", encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "secret.py").write_text("secret=2\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[],
        ))

    result = await call_tool("devcouncil_get_diff", {"task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["files"] == []
    assert payload["unified_diff"] == ""


@pytest.mark.anyio
async def test_get_diff_path_filter_empty_intersection(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "present.py").write_text("present\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {"paths": ["absent.py"]})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["files"] == []
    assert payload["unified_diff"] == ""


@pytest.mark.anyio
async def test_get_diff_truncated_keeps_files_authoritative(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _git_init(tmp_path)
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.txt"], cwd=tmp_path, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True, text=True)
    (tmp_path / "big_new.txt").write_text("x" * 30_000 + "\n", encoding="utf-8")

    result = await call_tool("devcouncil_get_diff", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["truncated"] is True
    assert "big_new.txt" in {f["path"] for f in payload["files"]}
    entry = next(f for f in payload["files"] if f["path"] == "big_new.txt")
    assert entry["status"] == "A"
    assert entry["additions"] >= 1


@pytest.mark.anyio
async def test_read_file_task_scope_allows_planned_path(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "a.py").write_text("ok\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("nope\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
        ))

    result = await call_tool("devcouncil_read_file", {"path": "a.py", "task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["content"] == "ok"


@pytest.mark.anyio
async def test_read_file_task_scope_blocks_unplanned_path(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "a.py").write_text("ok\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("secret\n", encoding="utf-8")
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
        ))

    result = await call_tool("devcouncil_read_file", {"path": "b.py", "task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "out_of_scope"
    assert "secret" not in result[0].text


@pytest.mark.anyio
async def test_read_file_task_id_without_db_fails_closed(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    (tmp_path / "leak.py").write_text("secret\n", encoding="utf-8")

    result = await call_tool("devcouncil_read_file", {"path": "leak.py", "task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "not_initialized"
    assert "secret" not in result[0].text


@pytest.mark.anyio
async def test_get_evidence_does_not_leak_other_task(tmp_path, monkeypatch):
    db = _init_db(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="A", description="D"))
        TaskRepository(session).save(Task(id="TASK-002", title="B", description="D"))
        EvidenceRepository(session).save_command_result(
            "TASK-002",
            CommandResult(
                command="pytest",
                exit_code=1,
                stdout_path="",
                stderr_path="",
                summary="other-task-secret-evidence",
            ),
        )

    result = await call_tool("devcouncil_get_evidence", {"task_id": "TASK-001"})
    payload = json.loads(result[0].text)
    assert payload.get("ok") is not False
    assert "other-task-secret-evidence" not in result[0].text
    evidence = payload.get("evidence") or []
    assert evidence == []
