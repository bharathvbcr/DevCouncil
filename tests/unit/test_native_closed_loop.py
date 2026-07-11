"""End-to-end parity: the native executor drives the SAME lease-gated write path and
shared verify/next-actions repair contract the MCP surface uses.

Mirrors ``tests/unit/test_mcp_closed_loop.py`` for the native executor:
checkout (implicit, via the agent) -> write through ``execution/gated_write.py`` ->
verify -> ``split_next_actions`` repair guidance -> re-verify -> pass, proving the
native path no longer bypasses the lease/scope gate and repairs identically to MCP.
Also covers execution.command_timeout and the docker/nix sandbox parity.
"""

import subprocess
import sys
import time

import pytest

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.permissions import PermissionManager, PermissionPolicy
from devcouncil.execution.task_runner import TaskRunner
from devcouncil.executors.native.agent import AgentAction, NativeAgent, ToolCall
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import RequirementRepository, TaskRepository


def _git(root, *args):
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root, check=True, capture_output=True, text=True,
    )


def _setup(tmp_path, *, allowed_commands=None):
    # A real git repo so the verifier can capture a working-tree diff.
    _git(tmp_path, "init")
    (tmp_path / "README.md").write_text("# x\n", encoding="utf-8")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-m", "init")

    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
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
            allowed_commands=list(allowed_commands or []),
            # Must be acceptance-capable evidence (mirrors the MCP loop test): the command
            # must actually exercise the written code, not trivia like `python --version`.
            expected_tests=[f'{sys.executable} -c "exec(open(\\"src/a.py\\").read()); assert VALUE == 1"'],
        ))
    return db


class _ScriptedRouter:
    """Minimal ModelRouter stand-in that plays back a fixed list of AgentActions."""

    def __init__(self, actions):
        self._actions = list(actions)
        self.calls = 0
        self.seen_messages: list[list[dict]] = []

    async def complete_structured(self, *, role, messages, schema):
        self.seen_messages.append([dict(m) for m in messages])
        action = self._actions[self.calls]
        self.calls += 1
        return action


def _agent(tmp_path, router, *, allowed_commands=None, sandbox="local"):
    policy = PermissionPolicy(allowed_shell_commands=list(allowed_commands or []))
    runner = TaskRunner(tmp_path, PermissionManager(policy, tmp_path))
    return NativeAgent(router, runner, sandbox=sandbox)


def _task(db):
    with db.get_session() as session:
        return TaskRepository(session).get_by_id("TASK-001")


def _reqs(db):
    with db.get_session() as session:
        return RequirementRepository(session).get_all()


def _write_action(content):
    return AgentAction(
        thought="write",
        tool_calls=[ToolCall(tool="apply_patch", args={"path": "src/a.py", "content": content})],
    )


def test_native_blocks_without_work_then_passes_after_gated_write(tmp_path):
    db = _setup(tmp_path)
    # finish (no work) -> BLOCKED -> repair guidance -> gated write -> finish -> PASS.
    router = _ScriptedRouter([
        AgentAction(thought="ship it", finish=True),
        _write_action("VALUE = 1\n"),
        AgentAction(thought="now done", finish=True),
    ])
    agent = _agent(tmp_path, router)

    result = agent.run_task(_task(db), _reqs(db))

    assert result.success is True
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 1\n"
    # The agent must have been handed the shared next-actions repair contract after the
    # first (work-free) verify BLOCKED it — identical guidance to the MCP surface.
    handed_back = any(
        "[Verification BLOCKED]" in (m.get("content") or "")
        for turn in router.seen_messages for m in turn
    )
    assert handed_back
    # Provenance records the gated write (proves it went through execution/gated_write.py).
    from devcouncil.storage.native import FileChangeRepository

    with db.get_session() as session:
        changes = FileChangeRepository(session).list_for_task("TASK-001")
    assert any(fc.path == "src/a.py" and fc.allowed for fc in changes)


def test_native_repair_after_failing_evidence(tmp_path):
    db = _setup(tmp_path)
    # write wrong -> finish -> BLOCKED -> write right -> finish -> PASS.
    router = _ScriptedRouter([
        _write_action("VALUE = 0\n"),
        AgentAction(thought="done?", finish=True),
        _write_action("VALUE = 1\n"),
        AgentAction(thought="done now", finish=True),
    ])
    agent = _agent(tmp_path, router)

    result = agent.run_task(_task(db), _reqs(db))

    assert result.success is True
    assert (tmp_path / "src" / "a.py").read_text() == "VALUE = 1\n"
    assert router.calls == 4


def test_native_gated_write_rejects_out_of_scope_file(tmp_path):
    db = _setup(tmp_path)
    agent = _agent(tmp_path, _ScriptedRouter([]))
    # Acquire a real lease the same way the loop does, then attempt an unplanned write.
    lease = agent._acquire_lease(_task(db))
    assert lease["ok"]
    agent._lease_token = lease["lease_token"]
    try:
        with pytest.raises(Exception) as excinfo:
            agent._gated_write_file(_task(db), "src/evil.py", "x = 1\n")
        assert "reject" in str(excinfo.value).lower()
        assert not (tmp_path / "src" / "evil.py").exists()
    finally:
        agent._release_lease(_task(db), agent._lease_token)


def test_native_run_command_honors_command_timeout(tmp_path):
    sleep_cmd = f'{sys.executable} -c "import time; time.sleep(5)"'
    db = _setup(tmp_path, allowed_commands=[sleep_cmd])
    router = _ScriptedRouter([
        AgentAction(thought="run", tool_calls=[ToolCall(tool="run_command", args={"command": sleep_cmd})]),
        AgentAction(thought="stop", finish=True),
    ])
    agent = _agent(tmp_path, router, allowed_commands=[sleep_cmd])
    # Squeeze the shared command_timeout knob so a 5s sleep must be killed early.
    assert agent.task_runner.config is not None
    agent.task_runner.config.execution.command_timeout = 1
    # Keep the finish step cheap: verification is not what this test exercises.
    agent._verify_task = lambda task, requirements: (True, [], [])

    started = time.monotonic()
    result = agent.run_task(_task(db), _reqs(db))
    elapsed = time.monotonic() - started

    assert result.success is True
    # The command was killed at ~1s, not allowed to run its full 5s sleep.
    assert elapsed < 5
    timed_out = any(
        "run_command" in (m.get("content") or "") and "Tool Error" in (m.get("content") or "")
        for turn in router.seen_messages for m in turn
    )
    assert timed_out


def test_native_verify_routes_through_sandbox_for_parity(tmp_path, monkeypatch):
    db = _setup(tmp_path)

    class _FakeSandboxResult:
        status = "passed"

    class _FakeSandbox:
        def __init__(self):
            self.ran = False

        def run(self, task, commands, requirements):
            self.ran = True
            return _FakeSandboxResult()

    fake = _FakeSandbox()
    seen = {}

    def _fake_get_sandbox(name, project_root):
        seen["name"] = name
        return fake

    monkeypatch.setattr("devcouncil.verification.sandbox.get_sandbox", _fake_get_sandbox)
    agent = _agent(tmp_path, _ScriptedRouter([]), sandbox="docker")

    passed, blocking, advisory = agent._verify_task(_task(db), _reqs(db))

    assert passed is True and blocking == [] and advisory == []
    assert fake.ran is True
    assert seen["name"] == "docker"


def test_native_verify_reports_unsupported_sandbox(tmp_path, monkeypatch):
    db = _setup(tmp_path)

    class _Unsupported:
        status = "unsupported"

    class _FakeSandbox:
        def run(self, task, commands, requirements):
            return _Unsupported()

    monkeypatch.setattr(
        "devcouncil.verification.sandbox.get_sandbox", lambda name, root: _FakeSandbox()
    )
    agent = _agent(tmp_path, _ScriptedRouter([]), sandbox="nix")

    passed, blocking, advisory = agent._verify_task(_task(db), _reqs(db))

    assert passed is False
    assert blocking and "unavailable" in blocking[0]["action"].lower()
