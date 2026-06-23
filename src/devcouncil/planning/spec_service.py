from typing import List
from pydantic import BaseModel
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.assumption import Assumption
from devcouncil.llm.router import ModelRouter

class BlockingQuestion(BaseModel):
    id: str
    question: str
    reason: str

class SpecOutput(BaseModel):
    requirements: List[Requirement]
    assumptions: List[Assumption]
    blocking_questions: List[BlockingQuestion]

class SpecService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def generate_spec(self, goal: str, repo_map_json: str) -> SpecOutput:
        prompt = f"""
Goal: {goal}

Repository Map:
{repo_map_json}

Your task is to draft the initial software specification for this goal.
1. Identify functional and non-functional requirements.
2. Extract any assumptions you are making about the codebase or architecture.
3. List any blocking questions that the user must answer before implementation can proceed.

Each requirement MUST have clear, testable acceptance criteria with verification methods.
Be RIGOROUS about edge cases — a terse goal hides most of the real requirements.
For every behavior, add explicit acceptance criteria covering, where applicable:
- the normal/happy path with concrete example inputs and expected outputs;
- boundary and degenerate inputs (empty, single element, zero, negative, very large,
  duplicate, already-sorted vs. unsorted, min/max);
- invalid or malformed inputs and the EXACT expected error behavior (e.g. raises
  ValueError/TypeError) rather than silent or undefined behavior;
- non-mutation / no-unexpected-side-effects on inputs when the behavior is a pure
  transformation;
- correct result TYPE (e.g. float vs int) when it matters.
Prefer several small, individually-verifiable acceptance criteria over one vague one.

Acceptance criteria MUST assert observable BEHAVIOR — return values, raised exceptions,
output, or side effects on supplied data — not repository state or tooling. DevCouncil's
own gates enforce file scope, clean diffs, and planned-file limits, so do NOT write
criteria about `git status`/`--porcelain` output, the exact set of changed/created files,
`git show HEAD` byte/append-only contents, commit shape, or whether flake8/mypy/ruff/
eslint/tsc/npm pass. Never require a tool the repo is not already configured for. Use the
`static_check` verification method ONLY for behavior expressible as a runnable assertion
(an importable function's result or raised exception), never to mean "a linter runs clean"
or "these files exist". If a criterion genuinely cannot be proven by running code
(architecture choices, repo scope, "works without extra configuration", subjective
quality), give it verification_method "manual" — it will be surfaced for human review
rather than block the automated gate. Prefer rewriting such a criterion as a concrete
behavioral one whenever possible.

Each assumption MUST have a confidence and impact level.
"""
        messages = [
            {"role": "user", "content": prompt}
        ]
        
        return await self.router.complete_structured(
            role="spec_writer",
            messages=messages,
            schema=SpecOutput
        )
