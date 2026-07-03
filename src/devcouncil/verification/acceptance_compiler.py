"""Compile natural-language acceptance criteria into self-contained executable
checks that DevCouncil owns and runs.

This is the difference between trusting the planner/agent's word and gathering
real evidence: instead of running planner-authored ``expected_tests`` (which the
benchmark showed often reference tools or test files that do not exist), DevCouncil
derives one runnable check per acceptance criterion directly from the criterion
text and the code under review, then maps each check 1:1 to its criterion.
"""

from __future__ import annotations

import asyncio
import json
import logging
from typing import Dict, List

from pydantic import BaseModel

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.llm.router import ModelRouter

logger = logging.getLogger(__name__)


class CompiledCheck(BaseModel):
    acceptance_criterion_id: str
    command: str  # a single shell command that exits 0 iff the criterion holds


class CompiledChecks(BaseModel):
    checks: List[CompiledCheck]


# A worked example pinned to the prompt. Weak/local models follow a concrete
# AC->command pairing far more reliably than rules alone — it anchors the import
# style, the exception-guard idiom, and "assert behavior, not state".
_WORKED_EXAMPLE = """Worked example (follow this shape exactly):
  Acceptance criterion: "median([]) raises ValueError"
  Code under review: a new file `stats.py` defining `def median(values): ...`
  CORRECT command:
    python -c "import stats\\ntry: stats.median([])\\nexcept ValueError: pass\\nelse: raise SystemExit(1)"
  Why: it imports the REAL module name implied by the file (stats.py -> import stats),
  exercises the public function, and asserts the OBSERVABLE behavior (a raised
  exception) — not any file/git state."""


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
        caller can fall back to the task's declared expected_tests. Single-shot
        wrapper over :meth:`compile_candidates` (samples=1) for back-compat.
        """
        return await self.compile_candidates(task, requirements, code_context, samples=1)

    async def compile_candidates(
        self,
        task: Task,
        requirements: List[Requirement],
        code_context: str,
        samples: int = 1,
        per_criterion: bool = False,
    ) -> Dict[str, List[str]]:
        """Return {acceptance_criterion_id: [up to ``samples`` INDEPENDENT check commands]}.

        Each criterion gets several independently-generated behavioral checks so the
        caller can decide by majority vote — outvoting a single mis-generated check
        without auto-passing a real defect. ``samples=1`` yields one command per
        criterion (identical to the original single-shot ``compile``). Local sampling
        is cost-free, so a weak model benefits most from a higher count.

        ``per_criterion`` compiles ONE criterion per model call instead of batching them
        all into a single prompt. A weak model batching N criteria into one JSON response
        routinely omits or mis-attributes some — producing false ``incomplete`` verdicts —
        whereas a focused single-criterion prompt is far more reliable. It costs N× the
        calls (cheap on a local monitor), so it is opt-in.
        """
        ac_by_id = {ac.id: ac for req in requirements for ac in req.acceptance_criteria}
        target = [ac_by_id[i] for i in task.acceptance_criterion_ids if i in ac_by_id]
        if not target:
            return {}

        if per_criterion and len(target) > 1:
            # Independent per-criterion compiles: run them CONCURRENTLY. Each call is
            # independent (own prompt, own criterion), so wall-clock drops from N
            # sequential model calls to max-of-N — the difference between minutes and
            # tens of minutes on a slow local monitor. A server that processes
            # requests serially just queues them (no worse than before).
            results = await asyncio.gather(
                *(self._sample_checks([ac], code_context, samples) for ac in target)
            )
            out: Dict[str, List[str]] = {}
            for result in results:
                out.update(result)
            return out
        return await self._sample_checks(target, code_context, samples)

    async def _sample_checks(
        self, target: List, code_context: str, samples: int
    ) -> Dict[str, List[str]]:
        """Generate up to ``samples`` independent check commands for ``target`` criteria."""
        valid_ids = {ac.id for ac in target}
        out: Dict[str, List[str]] = {}

        # Independent attempts. Each varies temperature + an attempt marker so the router
        # cache keys differ (otherwise an identical prompt+temp returns the cached answer
        # and every "sample" is the same command). Attempt 0 stays deterministic (temp 0).
        async def _one_attempt(attempt: int) -> "CompiledChecks":
            temperature = 0.0 if attempt == 0 else min(0.8, 0.3 + 0.2 * attempt)
            return await self.router.complete_structured(
                role=self.role,
                messages=[{"role": "user", "content": self._compile_prompt(target, code_context, attempt)}],
                schema=CompiledChecks,
                temperature=temperature,
                fallback=CompiledChecks(checks=[]),
            )

        # Attempts are independent by construction (distinct prompts/temperatures), so
        # run them concurrently; results are merged in attempt order to keep candidate
        # ordering deterministic. A failed attempt costs only itself.
        results = await asyncio.gather(
            *(_one_attempt(a) for a in range(max(1, samples))), return_exceptions=True
        )
        for attempt, result in enumerate(results):
            if isinstance(result, BaseException):
                logger.warning("Acceptance-check compile attempt %d failed: %s", attempt, result)
                continue
            for check in result.checks:
                cmd = (check.command or "").strip()
                if check.acceptance_criterion_id in valid_ids and cmd:
                    bucket = out.setdefault(check.acceptance_criterion_id, [])
                    if cmd not in bucket:  # dedup identical candidates across attempts
                        bucket.append(cmd)
        logger.info(
            "Compiled acceptance checks for %d/%d criteria (%d samples)",
            len(out), len(valid_ids), max(1, samples),
        )
        return out

    async def repair(
        self,
        ac_id: str,
        ac_description: str,
        failing_command: str,
        error_summary: str,
        code_context: str,
    ) -> str | None:
        """Regenerate a compiled check that FAILED TO RUN (malformed/unrunnable).

        Given the broken command and the launcher error, ask the model to fix the
        COMMAND so it runs against the code — never to change what it asserts. Returns
        the repaired command, or None if the model cannot produce a usable one. Safe by
        construction: a check that did not run proves nothing, so regenerating it cannot
        weaken the gate (a repaired check still has to genuinely pass to count)."""
        prompt = f"""The following DevCouncil acceptance check FAILED TO RUN — it is malformed or
its tooling/import is wrong, so it proves nothing about the code. Fix the COMMAND so it
RUNS and correctly tests the SAME criterion. Do NOT weaken or change what it asserts;
only fix what stops it from running (wrong module/import name, broken Python one-liner
syntax, unavailable tool).

Acceptance criterion ({ac_id}): {ac_description}

Failing command:
{failing_command}

Launcher error / output:
{error_summary}

Code under review:
{code_context}

{_WORKED_EXAMPLE}

Return JSON: one CompiledCheck with acceptance_criterion_id={ac_id!r} and the corrected
single self-contained command. If you cannot produce a runnable behavioral command,
return an empty checks list."""
        try:
            result = await self.router.complete_structured(
                role=self.role,
                messages=[{"role": "user", "content": prompt}],
                schema=CompiledChecks,
                fallback=CompiledChecks(checks=[]),
            )
        except Exception:
            return None
        for check in result.checks:
            cmd = (check.command or "").strip()
            if check.acceptance_criterion_id == ac_id and cmd and cmd != failing_command.strip():
                return cmd
        return None

    def _compile_prompt(self, target, code_context: str, attempt: int = 0) -> str:
        acs_json = json.dumps(
            [{"id": ac.id, "description": ac.description, "method": ac.verification_method} for ac in target],
            indent=2,
        )
        # Independent-attempt marker: nudges diversity across samples AND differentiates
        # the router cache key so a second sample is actually regenerated, not replayed.
        variant = "" if attempt == 0 else (
            f"\nIndependent attempt #{attempt}: derive each check FROM SCRATCH; do not assume "
            "a previous attempt's wording. Prefer a different but equivalent way to exercise "
            "the same behavior.\n"
        )
        prompt = f"""
You are DevCouncil's acceptance-test compiler. Convert each acceptance criterion
below into exactly ONE shell command that EXITS 0 if and only if the BEHAVIOR
described by the criterion holds for the code shown.
{variant}
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
- Choose DISCRIMINATING inputs: pick the input a plausible-but-lazy implementation
  would get WRONG, not the easiest happy-path example (which often passes on broken
  code and proves nothing). Examples: to prove "comment lines are skipped", the
  comment must CONTAIN the delimiter ('# c=3'); to prove "split on the first X",
  the value must itself contain X; to prove sorting/ordering, the input must start
  unsorted; to prove a boundary, test AT the boundary.

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

{_WORKED_EXAMPLE}
"""
        return prompt
