import asyncio
import re

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import Task
from devcouncil.verification.acceptance_compiler import (
    AcceptanceTestCompiler,
    CompiledCheck,
    CompiledChecks,
)


class _RecordingRouter:
    """Returns a check for every AC id present in the prompt, recording each prompt so a
    test can see how many calls were made and how many criteria each call carried."""

    def __init__(self):
        self.prompts = []

    async def complete_structured(self, role, messages, schema, temperature=None, fallback=None, **kw):
        prompt = messages[0]["content"]
        self.prompts.append(prompt)
        ids = re.findall(r'"id": "(AC-\d+)"', prompt)
        return CompiledChecks(checks=[
            CompiledCheck(acceptance_criterion_id=i, command=f'python -c "assert {i!r}"') for i in ids
        ])


def _task_and_req(n):
    acs = [AcceptanceCriterion(id=f"AC-{i}", description=f"behavior {i}", verification_method="unit_test")
           for i in range(1, n + 1)]
    req = Requirement(id="REQ-1", title="t", description="d", priority="high", source="user",
                      acceptance_criteria=acs)
    task = Task(id="T1", title="t", description="d", requirement_ids=["REQ-1"],
                acceptance_criterion_ids=[ac.id for ac in acs])
    return task, req


def test_compile_per_criterion_makes_one_focused_call_per_ac():
    task, req = _task_and_req(3)
    router = _RecordingRouter()
    out = asyncio.run(AcceptanceTestCompiler(router).compile_candidates(
        task, [req], "diff", samples=1, per_criterion=True))
    assert set(out) == {"AC-1", "AC-2", "AC-3"}
    assert len(router.prompts) == 3  # one model call per criterion
    for prompt in router.prompts:
        assert len(re.findall(r'"id": "AC-', prompt)) == 1  # each call carries exactly one AC


def test_compile_batched_is_a_single_call_by_default():
    task, req = _task_and_req(3)
    router = _RecordingRouter()
    out = asyncio.run(AcceptanceTestCompiler(router).compile_candidates(
        task, [req], "diff", samples=1))  # per_criterion defaults False
    assert set(out) == {"AC-1", "AC-2", "AC-3"}
    assert len(router.prompts) == 1  # all criteria in one batched call (unchanged behavior)


def test_compile_per_criterion_with_single_ac_stays_one_call():
    # With a single criterion there is nothing to decompose -> one call either way.
    task, req = _task_and_req(1)
    router = _RecordingRouter()
    asyncio.run(AcceptanceTestCompiler(router).compile_candidates(
        task, [req], "diff", samples=1, per_criterion=True))
    assert len(router.prompts) == 1
