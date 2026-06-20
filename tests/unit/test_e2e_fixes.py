"""Regression tests for the e2e-surfaced fixes:
reconciliation-on-blocked, plan-gate read-only, router JSON extraction, native resilience.
"""

import asyncio

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.executor import ExecutionResult
from devcouncil.gating.policy import GatePolicy
from devcouncil.llm.router import ModelRouter, StructuredOutputError


# ---------------------------------------------------------------- router._extract_json

def test_extract_json_from_prose():
    assert ModelRouter._extract_json('Here you go: {"value": "x"} thanks') == '{"value": "x"}'


def test_extract_json_from_fence():
    assert ModelRouter._extract_json('```json\n{"value": "x"}\n```') == '{"value": "x"}'


def test_extract_json_brace_inside_string_value():
    # The balanced scanner must not stop at a } that lives inside a string value.
    assert ModelRouter._extract_json('prefix {"value": "a}b"} suffix') == '{"value": "a}b"}'


def test_extract_json_passthrough_clean():
    assert ModelRouter._extract_json('{"a": 1}') == '{"a": 1}'


# ---------------------------------------------------------------- plan gate: read-only task

def _req():
    return Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
    )


def test_plan_gate_flags_read_only_only_task_advisory():
    task = Task(
        id="TASK-001", title="T", description="d", requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[PlannedFile(path="src/a.py", reason="inspect", allowed_change="read_only")],
    )
    result = GatePolicy().check_plan_approval([_req()], [task])
    readonly = [g for g in result.gaps if g.id.endswith("READ-ONLY") and g.id.startswith("GAP-PLAN-")]
    assert readonly and readonly[0].blocking is False  # advisory, not blocking
    assert result.passed is True  # an otherwise-valid plan still passes


def test_plan_gate_no_flag_for_writable_task():
    task = Task(
        id="TASK-001", title="T", description="d", requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[PlannedFile(path="src/a.py", reason="logic", allowed_change="modify")],
    )
    result = GatePolicy().check_plan_approval([_req()], [task])
    assert not [g for g in result.gaps if g.id.endswith("-READ-ONLY")]


# ---------------------------------------------------------------- native agent resilience

class _Router:
    """Minimal stand-in for ModelRouter."""
    def __init__(self, behavior):
        self._behavior = behavior
        self.calls = 0

    async def complete_structured(self, **kwargs):
        self.calls += 1
        return self._behavior(self.calls)


class _TaskRunner:
    def __init__(self, tmp_path, apply_raises=False):
        self.project_root = tmp_path
        self.apply_raises = apply_raises
        self.written = []

    def apply_patch(self, patch, task):
        if self.apply_raises:
            from devcouncil.app.errors import ExecutionError
            raise ExecutionError("Patch does not declare any affected files.")
        return True

    def write_file(self, path, content, task):
        self.written.append((path, content))

    def run_command(self, command, task):
        raise AssertionError("not used")


def _native(router, runner):
    from devcouncil.executors.native.agent import NativeAgent
    agent = NativeAgent.__new__(NativeAgent)
    agent.router = router
    agent.task_runner = runner
    from devcouncil.execution.context_builder import ContextBuilder
    agent.context_builder = ContextBuilder(runner.project_root)
    return agent


def _task():
    return Task(id="TASK-001", title="T", description="d",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])


def test_native_agent_structured_error_does_not_propagate(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.planning.correction_manifest.load_latest_correction_manifest",
                        lambda root, tid: None)
    def always_raise(n):
        raise StructuredOutputError("bad json", role="native_agent", model="m")
    agent = _native(_Router(always_raise), _TaskRunner(tmp_path))
    monkeypatch.setattr(agent.context_builder, "build_task_context", lambda t, r: "{}")
    result = asyncio.run(agent._run_task_async(_task(), []))
    assert isinstance(result, ExecutionResult) and result.success is False  # no propagation


def test_native_agent_aborts_on_repeated_patch_failures(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.planning.correction_manifest.load_latest_correction_manifest",
                        lambda root, tid: None)
    from devcouncil.executors.native.agent import AgentAction, ToolCall
    def emit_bad_patch(n):
        return AgentAction(thought="patch", tool_calls=[ToolCall(tool="apply_patch", args={"patch": "garbage"})])
    runner = _TaskRunner(tmp_path, apply_raises=True)
    agent = _native(_Router(emit_bad_patch), runner)
    monkeypatch.setattr(agent.context_builder, "build_task_context", lambda t, r: "{}")
    result = asyncio.run(agent._run_task_async(_task(), []))
    assert result.success is False
    assert "patch" in result.message.lower()  # aborted on patch failures, not step cap


def test_native_agent_path_content_fallback_writes_file(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.planning.correction_manifest.load_latest_correction_manifest",
                        lambda root, tid: None)
    from devcouncil.executors.native.agent import AgentAction, ToolCall
    runner = _TaskRunner(tmp_path)
    steps = [
        AgentAction(thought="write", tool_calls=[ToolCall(tool="apply_patch", args={"path": "src/a.py", "content": "X=1\n"})]),
        AgentAction(thought="done", finish=True),
    ]
    agent = _native(_Router(lambda n: steps[n - 1]), runner)
    monkeypatch.setattr(agent.context_builder, "build_task_context", lambda t, r: "{}")
    result = asyncio.run(agent._run_task_async(_task(), []))
    assert result.success is True
    assert runner.written == [("src/a.py", "X=1\n")]  # fallback routed through write_file


# ---------------------------------------------------------------- diagnostics truncation

def test_summarize_stream_surfaces_error_within_first_500_chars():
    from devcouncil.verification.verifier import Verifier
    # Large stdout + a stderr ending in the real error: the error must survive the
    # downstream summary[:500] clip (it previously did not).
    stdout = "x" * 4000
    stderr = (
        'Traceback (most recent call last):\n  File "<string>", line 1, in <module>\n'
        "AssertionError: ['.devcouncil/config.yaml', 'src/a.py']"
    )
    v = Verifier.__new__(Verifier)
    summary = (
        f"Exit code 1. "
        f"stderr: {v._summarize_stream(stderr)}. "
        f"stdout: {v._summarize_stream(stdout)}"
    )
    assert "AssertionError" in summary[:500]  # the actual error is diagnosable


def test_summarize_stream_handles_empty():
    from devcouncil.verification.verifier import Verifier
    assert Verifier._summarize_stream("") == "(empty)"
    assert Verifier._summarize_stream("   ") == "(empty)"


# ---------------------------------------------------------------- plan gate: overlapping ownership

def test_plan_gate_flags_overlapping_file_ownership():
    from devcouncil.domain.requirement import Requirement, AcceptanceCriterion
    from devcouncil.domain.task import Task, PlannedFile
    req = Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
    )
    # Two tasks both writing src/mathutils.py -> over-decomposition (advisory).
    common = dict(requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001"])
    t1 = Task(id="T-001", title="a", description="d",
              planned_files=[PlannedFile(path="src/mathutils.py", reason="add", allowed_change="modify")], **common)
    t2 = Task(id="T-002", title="b", description="d",
              planned_files=[PlannedFile(path="src/mathutils.py", reason="more", allowed_change="modify")], **common)
    result = GatePolicy().check_plan_approval([req], [t1, t2])
    overlap = [g for g in result.gaps if "OVERLAP" in g.id]
    assert overlap and overlap[0].blocking is False
    assert "src/mathutils.py" in overlap[0].description
    assert result.passed is True  # advisory only


def test_plan_gate_no_overlap_for_distinct_files():
    from devcouncil.domain.requirement import Requirement, AcceptanceCriterion
    from devcouncil.domain.task import Task, PlannedFile
    req = Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
    )
    common = dict(requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001"])
    t1 = Task(id="T-001", title="a", description="d",
              planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")], **common)
    t2 = Task(id="T-002", title="b", description="d",
              planned_files=[PlannedFile(path="tests/test_a.py", reason="x", allowed_change="create")], **common)
    result = GatePolicy().check_plan_approval([req], [t1, t2])
    assert not [g for g in result.gaps if "OVERLAP" in g.id]


# ---------------------------------------------------------------- native agent conversation accumulation

def test_native_agent_records_action_and_nudges_on_empty(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.planning.correction_manifest.load_latest_correction_manifest",
                        lambda root, tid: None)
    from devcouncil.executors.native.agent import AgentAction
    seen = []

    class R:
        def __init__(self): self.calls = 0
        async def complete_structured(self, **kw):
            self.calls += 1
            seen.append(list(kw["messages"]))
            if self.calls == 1:
                return AgentAction(thought="thinking")  # no tool_calls, not finished -> nudge
            return AgentAction(thought="done", finish=True)

    agent = _native(R(), _TaskRunner(tmp_path))
    monkeypatch.setattr(agent.context_builder, "build_task_context", lambda t, r: "{}")
    result = asyncio.run(agent._run_task_async(_task(), []))
    assert result.success is True
    # The 2nd model call must see the assistant's own step-1 action AND the nudge.
    second = seen[1]
    assert any(m["role"] == "assistant" for m in second)
    assert any("no tool_calls" in (m.get("content") or "") for m in second)


def test_native_agent_empty_actions_bounded_by_step_cap(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.planning.correction_manifest.load_latest_correction_manifest",
                        lambda root, tid: None)
    from devcouncil.executors.native.agent import AgentAction, MAX_AGENT_STEPS

    class R:
        def __init__(self): self.calls = 0
        async def complete_structured(self, **kw):
            self.calls += 1
            return AgentAction(thought="still thinking")  # never acts, never finishes

    r = R()
    agent = _native(r, _TaskRunner(tmp_path))
    monkeypatch.setattr(agent.context_builder, "build_task_context", lambda t, r2: "{}")
    result = asyncio.run(agent._run_task_async(_task(), []))
    assert result.success is False
    assert r.calls == MAX_AGENT_STEPS  # bounded; no infinite spin
