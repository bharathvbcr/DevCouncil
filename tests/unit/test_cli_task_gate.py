import json
import sys
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.native import TaskLeaseRepository
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()

def _setup_gate_env(tmp_path: Path, monkeypatch) -> tuple[Path, str, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    
    # Initialize DevCouncil
    init_res = runner.invoke(app, ["init"])
    assert init_res.exit_code == 0

    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Sample Task",
        description="d",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
        allowed_commands=["*python*"],
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        lease = TaskLeaseRepository(session).acquire(
            "TASK-1", owner="test", agent="test", ttl_seconds=600,
        )
        lease_tok = lease.lease_token
    return tmp_path, "TASK-1", lease_tok


def test_cli_next_task(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)
    
    # Test next-task when current task is leased (should return None/no task available)
    res1 = runner.invoke(app, ["next-task", "--json"])
    assert res1.exit_code == 0
    data1 = json.loads(res1.output)
    assert data1["ok"] is True
    assert data1["task"] is None

    # Add an unleased task
    db = Database(root / ".devcouncil" / "state.sqlite")
    task2 = Task(
        id="TASK-2",
        title="Another Task",
        description="d2",
        status="planned",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task2)

    res2 = runner.invoke(app, ["next-task", "--json"])
    assert res2.exit_code == 0
    data2 = json.loads(res2.output)
    assert data2["ok"] is True
    assert data2["task"]["id"] == "TASK-2"

    # Non-json format
    res3 = runner.invoke(app, ["next-task"])
    assert res3.exit_code == 0
    assert "TASK-2" in res3.output


def test_cli_scope_update(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)
    
    # Create the caller file to allow appending it to planned files
    caller = root / "src" / "caller.py"
    caller.parent.mkdir(parents=True, exist_ok=True)
    caller.write_text("x = 1\n", encoding="utf-8")

    res = runner.invoke(
        app,
        [
            "scope",
            "update",
            task_id,
            "--lease-token",
            lease_tok,
            "--allowed-command",
            "pytest tests/unit",
            "--expected-test",
            "pytest tests/unit",
            "--planned-file",
            "src/caller.py",
            "--json",
        ],
    )
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert "pytest tests/unit" in data["allowed_commands"]
    assert "pytest tests/unit" in data["expected_tests"]
    assert any(pf["path"] == "src/caller.py" for pf in data["planned_files"])

    # Non-json and fail cases
    res2 = runner.invoke(
        app,
        [
            "scope",
            "update",
            "INVALID-TASK",
            "--lease-token",
            lease_tok,
        ],
    )
    assert res2.exit_code != 0


def test_cli_policy_check(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)
    
    # Set status of task to running so policy check can infer it
    db = Database(root / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
        task.status = "running"
        TaskRepository(session).save(task)

    res = runner.invoke(app, ["policy-check", "src/a.py", "--json"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["allowed"] is True

    # Non-json format
    res2 = runner.invoke(app, ["policy-check", "src/a.py"])
    assert res2.exit_code == 0
    assert "allow" in res2.output or "warn" in res2.output


def test_cli_record_command(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)

    res = runner.invoke(
        app,
        [
            "record-command",
            task_id,
            "--lease-token",
            lease_tok,
            "--command",
            "pytest",
            "--status",
            "started",
            "--json",
        ],
    )
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert data["recorded"] is True

    # Non-json format and fail cases
    res2 = runner.invoke(
        app,
        [
            "record-command",
            task_id,
            "--lease-token",
            lease_tok,
            "--command",
            "pytest",
            "--status",
            "invalid-status",
        ],
    )
    assert res2.exit_code != 0


def test_cli_run_cmd(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)

    # Command is allowed (pytest)
    res = runner.invoke(
        app,
        [
            "run-cmd",
            task_id,
            "--lease-token",
            lease_tok,
            "--command",
            sys.executable + " --version",
            "--json",
        ],
    )
    assert res.exit_code == 0, f"res.output: {res.output}\nres.exception: {res.exception}"
    data = json.loads(res.output)
    assert data["ok"] is True

    # Command is not allowed
    res2 = runner.invoke(
        app,
        [
            "run-cmd",
            task_id,
            "--lease-token",
            lease_tok,
            "--command",
            "rm -rf /",
            "--json",
        ],
    )
    assert res2.exit_code == 0  # it returns payload saying code=command_not_allowed
    data2 = json.loads(res2.output)
    assert data2["ok"] is False
    assert data2["code"] == "command_not_allowed"


def test_cli_verify_leased(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)

    # docker sandbox unsupported
    res = runner.invoke(
        app,
        [
            "verify-leased",
            task_id,
            "--lease-token",
            lease_tok,
            "--sandbox",
            "docker",
        ],
    )
    assert res.exit_code != 0
    data = json.loads(res.output)
    assert data["ok"] is False
    assert data["code"] == "unsupported_sandbox"


def test_cli_evidence_append_and_list(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)

    res = runner.invoke(
        app,
        [
            "evidence-append",
            task_id,
            "--lease-token",
            lease_tok,
            "--command",
            "pytest",
            "--summary",
            "tests passed",
            "--exit-code",
            "0",
            "--json",
        ],
    )
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True

    # List evidence
    res2 = runner.invoke(app, ["evidence-list", task_id, "--json"])
    assert res2.exit_code == 0
    data2 = json.loads(res2.output)
    assert data2["ok"] is True
    assert len(data2["evidence"]) == 1
    assert data2["evidence"][0]["command"] == "pytest"

    # Non-json list
    res3 = runner.invoke(app, ["evidence-list", task_id])
    assert res3.exit_code == 0
    assert "pytest" in res3.output


def test_cli_handoff_leased(tmp_path, monkeypatch):
    root, task_id, lease_tok = _setup_gate_env(tmp_path, monkeypatch)

    res = runner.invoke(
        app,
        [
            "handoff-leased",
            task_id,
            "--lease-token",
            lease_tok,
            "--from",
            "worker",
            "--to",
            "reviewer",
            "--instruction",
            "please review",
            "--json",
        ],
    )
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert "manifest_path" in data
