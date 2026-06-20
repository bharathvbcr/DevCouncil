"""End-to-end: the full coding-companion loop composes over MCP alone.

checkout -> (write via MCP write path) -> verify -> resumable next-actions, proving the
headline guarantee: an agent that writes nothing is BLOCKED, and one that makes the
change through the lease-gated write path passes — all without touching the filesystem
or DB outside DevCouncil's gated tools.
"""

import json
import subprocess

import pytest

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.integrations.mcp.server import call_tool
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import RequirementRepository, TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _setup(tmp_path):
    # A real git repo so the verifier can capture a working-tree diff.
    _git(tmp_path, "init")
    (tmp_path / "README.md").write_text("# x\n", encoding="utf-8")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")

    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-001", title="R", description="d", priority="high", source="user",
            acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
        ))
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001"],
            planned_files=[PlannedFile(path="src/a.py", reason="logic", allowed_change="modify")],
            expected_tests=["python --version"],
        ))
    return db


async def _call(tool, args):
    return json.loads((await call_tool(tool, args))[0].text)


@pytest.mark.anyio
async def test_full_mcp_loop_blocks_without_work_then_passes_after_write(tmp_path, monkeypatch):
    _setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    checkout = await _call("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"})
    assert checkout["ok"] and checkout["expires_at"]  # lease with TTL
    token = checkout["lease_token"]

    # 1. Verify having written nothing -> blocked by the empty-diff guard.
    v1 = await _call("devcouncil_verify_task", {"task_id": "TASK-001", "lease_token": token})
    assert v1["passed"] is False
    assert v1["diff_empty"] is True
    assert any(g["gap_type"] == "task_not_implemented" for g in v1["blocking_gaps"])
    assert v1["verification_mode"] in {"coarse", "compiled"}

    # 2. The gap is resumable without re-verifying.
    na = await _call("devcouncil_get_next_actions", {"task_id": "TASK-001"})
    assert na["next_actions"], "blocking next action must be readable without re-verify"

    # 3. Make the change through the lease-gated MCP write path.
    w = await _call("devcouncil_write_file", {
        "task_id": "TASK-001", "lease_token": token, "path": "src/a.py", "content": "VALUE = 1\n",
    })
    assert w["ok"] and w["applied_files"] == ["src/a.py"]
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 1\n"

    # 4. Re-verify: real work present, passing command proves the criterion -> passes.
    v2 = await _call("devcouncil_verify_task", {"task_id": "TASK-001", "lease_token": token})
    assert v2["diff_empty"] is False
    assert v2["passed"] is True
    assert not any(g["gap_type"] == "task_not_implemented" for g in v2["blocking_gaps"])

    # 5. Provenance records the gated write; release frees the lease.
    prov = await _call("devcouncil_get_task_provenance", {"task_id": "TASK-001"})
    assert any(fc["path"] == "src/a.py" and fc["allowed"] for fc in prov["file_changes"])
    rel = await _call("devcouncil_release_task", {"task_id": "TASK-001", "lease_token": token})
    assert rel["ok"] and rel["released"] is True
