"""Golden fixture: certified Claude Code MCP hero loop phases.

Complements test_mcp_closed_loop.py with an explicit phase checklist agents and CI
can treat as the graduation gate for the hero loop.
"""

import json
import subprocess
import sys

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


def _golden_setup(tmp_path):
    _git(tmp_path, "init")
    (tmp_path / "README.md").write_text("# fixture\n", encoding="utf-8")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")

    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text("project:\n  name: golden\n", encoding="utf-8")
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-GOLD", title="Golden", description="d", priority="high", source="user",
            acceptance_criteria=[AcceptanceCriterion(id="AC-GOLD", description="t", verification_method="unit_test")],
        ))
        TaskRepository(session).save(Task(
            id="TASK-GOLD", title="Golden task", description="Set VALUE",
            requirement_ids=["REQ-GOLD"], acceptance_criterion_ids=["AC-GOLD"],
            planned_files=[PlannedFile(path="src/golden.py", reason="logic", allowed_change="modify")],
            expected_tests=[f'{sys.executable} -c "exec(open(\\"src/golden.py\\").read()); assert VALUE == 42"'],
        ))
    return db


async def _call(tool, args):
    return json.loads((await call_tool(tool, args))[0].text)


@pytest.mark.anyio
async def test_hero_loop_golden_phases(tmp_path, monkeypatch):
    """Certified path: checkout → blocked verify → write → pass → renew → release."""
    _golden_setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    checkout = await _call("devcouncil_checkout_task", {"task_id": "TASK-GOLD", "client_id": "golden-agent"})
    assert checkout["ok"] and checkout["expires_at"]
    token = checkout["lease_token"]

    blocked = await _call("devcouncil_verify_task", {"task_id": "TASK-GOLD", "lease_token": token})
    assert blocked["passed"] is False

    write = await _call("devcouncil_write_file", {
        "task_id": "TASK-GOLD", "lease_token": token,
        "path": "src/golden.py", "content": "VALUE = 42\n",
    })
    assert write["ok"]

    passed = await _call("devcouncil_verify_task", {"task_id": "TASK-GOLD", "lease_token": token})
    assert passed["passed"] is True

    renewed = await _call("devcouncil_renew_lease", {
        "task_id": "TASK-GOLD", "lease_token": token, "ttl_seconds": 120,
    })
    assert renewed["ok"] and renewed["expires_at"]

    released = await _call("devcouncil_release_task", {"task_id": "TASK-GOLD", "lease_token": token})
    assert released["ok"] and released["released"] is True


@pytest.mark.anyio
async def test_hero_loop_expired_lease_actionable(tmp_path, monkeypatch):
    """Expired leases return lease_expired with checkout guidance (not a generic invalid)."""
    from datetime import datetime, timedelta, timezone

    from devcouncil.storage.models import TaskLeaseModel
    from devcouncil.storage.native import TaskLeaseRepository

    _golden_setup(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    checkout = await _call("devcouncil_checkout_task", {"task_id": "TASK-GOLD", "client_id": "ttl-agent"})
    token = checkout["lease_token"]

    from devcouncil.storage.db import get_db

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        repo = TaskLeaseRepository(session)
        lease = repo.active_for_task("TASK-GOLD")
        assert lease is not None
        model = session.get(TaskLeaseModel, lease.id)
        model.expires_at = (datetime.now(timezone.utc) - timedelta(seconds=5)).isoformat()
        session.add(model)
        session.commit()

    verify = await _call("devcouncil_verify_task", {"task_id": "TASK-GOLD", "lease_token": token})
    assert verify.get("code") == "lease_expired"
    assert verify.get("suggested_tool") == "devcouncil_checkout_task"
