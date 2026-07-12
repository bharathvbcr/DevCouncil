import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _setup_write_env(tmp_path: Path, monkeypatch) -> tuple[Path, str, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    
    import subprocess
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    
    runner.invoke(app, ["init"])
    
    # Create task with a planned file src/a.py
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Write Task",
        description="d",
        status="planned",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        lease = TaskLeaseRepository(session).acquire(
            "TASK-1", owner="test", agent="test", ttl_seconds=600,
        )
        lease_tok = lease.lease_token
    return tmp_path, "TASK-1", lease_tok


def test_cli_write_allowed_and_denied(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_write_env(tmp_path, monkeypatch)
    
    # Pre-create the file to satisfy the modify action requirement
    a_py = root / "src" / "a.py"
    a_py.parent.mkdir(parents=True, exist_ok=True)
    a_py.write_text("x = 1\n", encoding="utf-8")

    # Allowed write (to src/a.py)
    res = runner.invoke(
        app,
        [
            "write",
            task_id,
            "--lease-token",
            lease_tok,
            "--path",
            "src/a.py",
            "--content",
            "x = 2\n",
            "--json",
        ],
    )
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert a_py.read_text(encoding="utf-8") == "x = 2\n"

    # Denied write (to src/b.py - not in planned files)
    res2 = runner.invoke(
        app,
        [
            "write",
            task_id,
            "--lease-token",
            lease_tok,
            "--path",
            "src/b.py",
            "--content",
            "y = 1\n",
            "--json",
        ],
    )
    assert res2.exit_code != 0
    data2 = json.loads(res2.output)
    assert data2["ok"] is False


def test_cli_apply_patch_allowed_and_denied(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_write_env(tmp_path, monkeypatch)
    
    a_py = root / "src" / "a.py"
    a_py.parent.mkdir(parents=True, exist_ok=True)
    a_py.write_text("x = 1\n", encoding="utf-8")
    
    import subprocess
    subprocess.run(["git", "add", "src/a.py"], cwd=root, capture_output=True)

    # Allowed patch (to src/a.py)
    diff = (
        "diff --git a/src/a.py b/src/a.py\n"
        "--- a/src/a.py\n"
        "+++ b/src/a.py\n"
        "@@ -1,1 +1,1 @@\n"
        "-x = 1\n"
        "+x = 42\n"
    )
    res = runner.invoke(
        app,
        [
            "apply-patch",
            task_id,
            "--lease-token",
            lease_tok,
            "--unified-diff",
            diff,
            "--json",
        ],
    )
    assert res.exit_code == 0, f"res.output: {res.output}\nres.exception: {res.exception}"
    data = json.loads(res.output)
    assert data["ok"] is True
    assert a_py.read_text(encoding="utf-8") == "x = 42\n"

    # Denied patch (modifying src/b.py - not in planned files)
    diff_denied = (
        "--- src/b.py\n"
        "+++ src/b.py\n"
        "@@ -1,1 +1,1 @@\n"
        "-y = 1\n"
        "+y = 2\n"
    )
    res2 = runner.invoke(
        app,
        [
            "apply-patch",
            task_id,
            "--lease-token",
            lease_tok,
            "--unified-diff",
            diff_denied,
            "--json",
        ],
    )
    assert res2.exit_code != 0
    data2 = json.loads(res2.output)
    assert data2["ok"] is False
