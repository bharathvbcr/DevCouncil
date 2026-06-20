"""Compile natural-language acceptance criteria into self-contained executable
checks that DevCouncil owns and runs.

This is the difference between trusting the planner/agent's word and gathering
real evidence: instead of running planner-authored ``expected_tests`` (which the
benchmark showed often reference tools or test files that do not exist), DevCouncil
derives one runnable check per acceptance criterion directly from the criterion
text and the code under review, then maps each check 1:1 to its criterion.
"""

from __future__ import annotations

import json
from typing import Dict, List

from pydantic import BaseModel

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.llm.router import ModelRouter


class CompiledCheck(BaseModel):
    acceptance_criterion_id: str
    command: str  # a single shell command that exits 0 iff the criterion holds


class CompiledChecks(BaseModel):
    checks: List[CompiledCheck]


class AcceptanceTestCompiler:
    def __init__(self, router: ModelRouter, role: str = "implementation_reviewer"):
        self.router = router
        self.role = role

    async def compile(
        self,
        task: Task,
        requirements: List[Requirement],
        code_context: str,
    ) -> Dict[str, List[str]]:
        """Return {acceptance_criterion_id: [self-contained check command(s)]}.

        Best-effort: returns {} if the model cannot produce usable checks, so the
        caller can fall back to the task's declared expected_tests.
        """
        ac_by_id = {ac.id: ac for req in requirements for ac in req.acceptance_criteria}
        target = [ac_by_id[i] for i in task.acceptance_criterion_ids if i in ac_by_id]
        if not target:
            return {}

        acs_json = json.dumps(
            [{"id": ac.id, "description": ac.description, "method": ac.verification_method} for ac in target],
            indent=2,
        )
        prompt = f"""
You are DevCouncil's acceptance-test compiler. Convert each acceptance criterion
below into exactly ONE shell command that EXITS 0 if and only if the BEHAVIOR
described by the criterion holds for the code shown.

Acceptance criteria:
{acs_json}

Code under review (the diff / current files):
{code_context}

What a check must verify — BEHAVIOR ONLY:
- A check exists to confirm the code DOES what the criterion describes when its
  public API is exercised: import the module/symbol and call its function(s), or
  run its CLI/entrypoint, and assert on the observable result (return value,
  raised exception, stdout, exit code).
- DevCouncil already enforces scope, file ownership, and append-only/orphan-diff
  constraints with its OWN gates. Acceptance checks must therefore NEVER re-assert
  repository or filesystem STATE — that is not their job and it produces false
  BLOCKED results because `dev` itself adds workspace files (AGENTS.md, CLAUDE.md,
  .gitignore, .devcouncil/config.yaml, etc.).

Rules — the commands are executed verbatim by the verifier:
- One command per acceptance_criterion_id (reference the id exactly).
- Each command MUST be a single, SELF-CONTAINED, immediately-runnable command:
  import the real module/symbol from the code and assert the behavior directly.
  Do NOT depend on test files, fixtures, or any external setup.
- Prefer: python -c "import <module>; assert <expr>". For an expected exception,
  use a one-line guard, e.g.
  python -c "import m; \\ntry: m.f([])\\nexcept ValueError: pass\\nelse: raise SystemExit(1)"
  (real newlines are fine; never put try/if/for after a ';').
- Use the actual module name implied by the code (e.g. file 'stats.py' -> import stats).

HARD PROHIBITIONS — a command that does any of these is INVALID; omit the
criterion instead of emitting such a command:
- NEVER assert exact git or filesystem state. Forbidden: `git status`,
  `git status --porcelain`, `git diff`, `git diff --name-only`, `git show`,
  `git ls-files`, `ls`/`find`/`os.listdir` equality checks, asserting a precise
  set or count of changed/created files, or asserting a file does/does not exist
  as the criterion's pass condition.
- NEVER do append-only or byte-level file/content comparisons (e.g.
  `git show HEAD:file`, diffing bytes, asserting only N bytes/lines were added).
  Assert the resulting BEHAVIOR instead, not how the file changed.
- NEVER invoke linters, type checkers, formatters, or build/package tools that
  may be absent: flake8, mypy, ruff, pylint, black, isort, eslint, tsc, prettier,
  npm, npx, yarn, pnpm, cargo, go vet, etc. Only use such a tool if the code
  context clearly shows it is configured for this repo (e.g. a matching config
  section/file is present in the context) AND it is essential to the criterion.
- If a criterion cannot be checked by a behavioral command (e.g. pure 'manual'
  review, or it only describes repo/tooling state), OMIT it rather than inventing
  a state-based or bogus command.
"""
        try:
            result = await self.router.complete_structured(
                role=self.role,
                messages=[{"role": "user", "content": prompt}],
                schema=CompiledChecks,
                fallback=CompiledChecks(checks=[]),
            )
        except Exception:
            return {}

        out: Dict[str, List[str]] = {}
        valid_ids = {ac.id for ac in target}
        for check in result.checks:
            cmd = (check.command or "").strip()
            if check.acceptance_criterion_id in valid_ids and cmd:
                out.setdefault(check.acceptance_criterion_id, []).append(cmd)
        return out
