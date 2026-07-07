import json

import pytest

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.integrations.mcp.server import call_tool, list_tools
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, TaskRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_checkout_returns_prompt_scope_and_lease_token(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(id="TASK-001", title="T", description="D", allowed_commands=["pytest"])
        )
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool(
        "devcouncil_checkout_task",
        {"task_id": "TASK-001", "client_id": "cursor"},
    )
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["lease_token"]
    assert payload["prompt"]
    assert payload["planned_files"] == []
    assert payload["allowed_commands"] == ["pytest"]


@pytest.mark.anyio
async def test_checkout_rejects_second_active_lease(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"})
    second = await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "b"})
    payload = json.loads(second[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "lease_conflict"


@pytest.mark.anyio
async def test_release_rejects_wrong_token(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )
    released = await call_tool(
        "devcouncil_release_task",
        {"task_id": "TASK-001", "lease_token": "bad-token"},
    )
    payload = json.loads(released[0].text)
    assert payload["ok"] is False

    ok_release = await call_tool(
        "devcouncil_release_task",
        {"task_id": "TASK-001", "lease_token": checkout["lease_token"]},
    )
    assert json.loads(ok_release[0].text)["ok"] is True


@pytest.mark.anyio
async def test_append_evidence_requires_valid_token(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    denied = await call_tool(
        "devcouncil_append_evidence",
        {
            "task_id": "TASK-001",
            "lease_token": "bad",
            "command": "pytest",
            "exit_code": 0,
            "summary": "ok",
        },
    )
    assert json.loads(denied[0].text)["ok"] is False

    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )
    allowed = await call_tool(
        "devcouncil_append_evidence",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "command": "pytest",
            "exit_code": 0,
            "summary": "ok",
        },
    )
    assert json.loads(allowed[0].text)["ok"] is True


@pytest.mark.anyio
async def test_mcp_lists_checkout_tools():
    tools = {tool.name for tool in await list_tools()}
    assert "devcouncil_checkout_task" in tools
    assert "devcouncil_release_task" in tools


@pytest.mark.anyio
async def test_checkout_returns_not_initialized_without_db(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool(
        "devcouncil_checkout_task",
        {"task_id": "TASK-001", "client_id": "cursor"},
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "not_initialized"


@pytest.mark.anyio
async def test_update_scope_rejects_non_array_values(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    result = await call_tool(
        "devcouncil_update_task_scope",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "allowed_commands": "pytest",
        },
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["argument"] == "allowed_commands"


@pytest.mark.anyio
async def test_update_scope_rejects_trivial_self_cert_commands(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    result = await call_tool(
        "devcouncil_update_task_scope",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "expected_tests": ['python -c "print(\'ok\')"'],
        },
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["expected_tests"] == []
    assert payload["rejected_expected_tests"] == ['python -c "print(\'ok\')"']


@pytest.mark.anyio
async def test_update_scope_tracks_agent_appended_evidence_commands(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    cmd = "python -m pytest tests/test_app.py -q"
    result = await call_tool(
        "devcouncil_update_task_scope",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "expected_tests": [cmd],
        },
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert cmd in payload["expected_tests"]
    assert payload["rejected_expected_tests"] == []
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        assert task is not None
        assert cmd in task.agent_appended_expected_tests


@pytest.mark.anyio
async def test_mcp_verify_persists_gaps_evidence_and_task_status(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    def fake_verify_payload(project_root, *, task_id, lease_token, sandbox="local"):
        from devcouncil.integrations.mcp.util import allowed_next_tools
        from devcouncil.storage.db import get_db
        from devcouncil.storage.native import TaskLeaseRepository
        from devcouncil.storage.repositories import GapRepository, EvidenceRepository, TaskRepository
        from devcouncil.verification.next_actions import split_next_actions

        db = get_db(project_root)
        gaps = [
            Gap(
                id="GAP-1",
                severity="high",
                gap_type="test_failed",
                task_id=task_id,
                description="failed",
                recommended_fix="fix",
                blocking=True,
            )
        ]
        evidence = [
            CommandResult(
                command="pytest",
                exit_code=1,
                stdout_path="",
                stderr_path="",
                summary="failed",
            )
        ]
        with db.get_session() as session:
            if not TaskLeaseRepository(session).validate(task_id, lease_token):
                return {"ok": False, "error": "Invalid lease token.", "code": "invalid_lease", "task_id": task_id}
            GapRepository(session).delete_for_task(task_id)
            EvidenceRepository(session).delete_for_task(task_id)
            for gap in gaps:
                GapRepository(session).save(gap)
            for ev in evidence:
                EvidenceRepository(session).save_command_result(task_id, ev)
            task = TaskRepository(session).get_by_id(task_id)
            assert task is not None
            task.status = "blocked"
            TaskRepository(session).save(task)
        blocking = [g.model_dump() for g in gaps if g.blocking]
        blocking_actions, advisory_actions = split_next_actions(gaps)
        return {
            "ok": True,
            "task_id": task_id,
            "status": "blocked",
            "sandbox": sandbox,
            "blocking_gaps": blocking,
            "next_actions": [a.model_dump() for a in blocking_actions],
            "advisory_actions": [a.model_dump() for a in advisory_actions],
            "allowed_next_tools": allowed_next_tools("blocked", True),
            "passed": False,
            "verification_mode": "coarse",
            "compiler_active": False,
            "diff_empty": True,
            "coverage_measured": False,
            "coverage_skipped_reason": None,
            "difficulty": None,
            "rigor_applied": [],
        }

    import devcouncil.integrations.mcp.util as mcp_util
    from devcouncil.utils.json_persist import dump_json

    real_run_cli = mcp_util.run_cli_command

    def fake_run_cli(args, root):
        if args and args[0] == "verify-leased":
            payload = fake_verify_payload(
                root,
                task_id=args[1],
                lease_token=args[args.index("--lease-token") + 1],
                sandbox=args[args.index("--sandbox") + 1] if "--sandbox" in args else "local",
            )
            return {
                "ok": payload.get("ok", True),
                "returncode": 0 if payload.get("ok") else 1,
                "stdout": dump_json(payload, indent=2),
                "stderr": "",
                "stdout_truncated": False,
                "stderr_truncated": False,
                "timed_out": False,
            }
        return real_run_cli(args, root)

    monkeypatch.setattr(mcp_util, "run_cli_command", fake_run_cli)
    monkeypatch.setattr("devcouncil.integrations.mcp.handlers.verify.run_cli_command", fake_run_cli)
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    result = await call_tool(
        "devcouncil_verify_task",
        {"task_id": "TASK-001", "lease_token": checkout["lease_token"]},
    )

    payload = json.loads(result[0].text)
    assert payload["passed"] is False
    assert payload["status"] == "blocked"
    with db.get_session() as session:
        assert TaskRepository(session).get_by_id("TASK-001").status == "blocked"
        assert GapRepository(session).get_all()[0].id == "GAP-1"
        assert EvidenceRepository(session).get_command_results_for_task("TASK-001")[0].command == "pytest"


@pytest.mark.anyio
async def test_update_scope_filters_and_tracks_allowed_commands(tmp_path, monkeypatch):
    """Trivial allowed_commands are rejected; legitimate ones are appended AND
    recorded in agent_appended_allowed_commands so they can never coarse-prove."""
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-001", title="T", description="D"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = json.loads(
        (await call_tool("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a"}))[0].text
    )

    result = await call_tool(
        "devcouncil_update_task_scope",
        {
            "task_id": "TASK-001",
            "lease_token": checkout["lease_token"],
            "allowed_commands": ["echo done", "python --version", "make build"],
        },
    )

    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["rejected_allowed_commands"] == ["echo done", "python --version"]
    assert "make build" in payload["allowed_commands"]
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        assert task is not None
        assert "make build" in task.agent_appended_allowed_commands
        assert "echo done" not in task.allowed_commands


def test_task_difficulty_and_provenance_round_trip(tmp_path):
    """difficulty and agent-append provenance must survive a DB save/reload —
    otherwise the rigor policy and the self-certification guard silently degrade."""
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="D",
            difficulty="hard",
            agent_appended_expected_tests=["pytest tests/test_x.py"],
            agent_appended_allowed_commands=["make build"],
        ))
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        assert task is not None
        assert task.difficulty == "hard"
        assert task.agent_appended_expected_tests == ["pytest tests/test_x.py"]
        assert task.agent_appended_allowed_commands == ["make build"]
