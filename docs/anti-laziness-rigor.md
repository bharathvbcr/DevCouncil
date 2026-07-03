# Anti-Laziness Rigor: Design

Goal: catch and correct coding-agent laziness (stubs, undersized diffs, premature "done", repeated failed approaches) — especially on hard tasks. Strictness policy: **advisory everywhere, blocking on hard tasks**.

## 0. Shared foundation: task difficulty

New module `src/devcouncil/verification/difficulty.py`.

`estimate_difficulty(task, requirements) -> Literal["easy", "normal", "hard"]` — deterministic scoring, no LLM call:

| Signal | Points |
|---|---|
| planned_files: 3–4 / ≥5 | +1 / +2 |
| acceptance criteria: 3–4 / ≥5 | +1 / +2 |
| any `create` planned file alongside `modify` files | +1 |
| depends_on ≥ 2 | +1 |
| hard keywords in title/description/requirements (refactor, migrate/migration, concurren*, async, race, protocol, parser, cache, transaction, auth*, crypto*, distributed, backward-compat) | +1 (capped) |
| requirement priority critical/high on a linked requirement | +1 |

Score ≥4 → hard, ≥2 → normal, else easy. Manual override: new optional `Task.difficulty` field (`Literal["easy","normal","hard"] | None`); when set, the estimator is bypassed. Planner/council can set it; users can set it via task edit.

`RigorPolicy` derived from difficulty + config:

```yaml
verification:
  rigor:
    enabled: true
    # when a check below says "hard", it blocks only on hard tasks;
    # "always" / "never" also accepted
    stub_detection: hard
    effort_heuristics: hard
    enforce_coverage_on_hard: true    # flips diff_coverage.enforce for hard tasks
    reviewer_required_on_hard: false  # opt-in: implementation_reviewer pass on hard tasks
    extra_repair_attempts_on_hard: 1  # added to execution.max_repair_attempts
```

## 1. Stub / TODO detection gate (new verifier Gate 2.5)

New module `src/devcouncil/verification/stub_detector.py`; called from `Verifier.verify_task()` after orphan-diff detection. Operates **only on lines added in the task's diff** (never pre-existing code).

Detections:

- Python (AST on post-change file content, restricted to functions/classes whose body lines intersect added lines): body is only `pass` / `...` / `raise NotImplementedError` / docstring-only with no logic.
- Marker scan on added lines, all languages: `TODO`, `FIXME`, `XXX`, `HACK`, `stub`, `not implemented`, `implement later/me/this`.
- Skipped/neutered tests added in diff: `@pytest.mark.skip`/`skipif(True`, `@unittest.skip`, `it.skip`/`xit`/`test.skip`, `assert True  # placeholder`, commented-out asserts.
- JS/TS/Go/Rust regex: `throw new Error("not implemented")`, `todo!()`, `unimplemented!()`, `panic("TODO")`, empty exported function body `{}` (added lines only).

Each finding → `Gap(gap_type="stub_detected", file, line, severity="high")`. Blocking per policy (hard only, by default). Suppression: a line containing `devcouncil: allow-stub` is skipped **only when the task description mentions scaffolding** (e.g. "scaffolding"); every allow-stub marker is still surfaced as an advisory `stub_declared` gap for audit.

Plumbing: add `"stub_detected"` to `Gap.gap_type` Literal; `next_actions._CATEGORY_BY_GAP_TYPE["stub_detected"] = "fix_code"` with action text "Replace the stub/placeholder at {file}:{line} with a real implementation, then re-verify."; `correction_manifest._GAP_TYPE_PRIORITY["stub_detected"] = 1` (real-defect band, alongside acceptance gaps).

## 2. Effort / diff heuristics (new verifier Gate 2.6)

Same policy switch (`effort_heuristics`). New gap_type `suspicious_effort` (severity medium; category `review`). Heuristics, deliberately conservative:

- **Undersized diff vs scope**: task plans ≥3 non-read-only files or has a `create` file, but total added lines < 5 per planned changed file (tunable `min_added_lines_per_planned_file: 5`). Skipped when the diff is empty (Gate 1 `task_not_implemented` already covers that).
- **Comment/whitespace-only diff**: >0 changed files but zero added lines that are code (after stripping comments/blank lines) while ACs have automatable verification methods.
- **Test-deletion**: diff removes more test-file lines than it adds while `expected_tests` reference those files — classic "make the suite pass by deleting the test". Severity high, and blocking even at `hard` policy level only on hard tasks, per the global rule.

These stay advisory on easy/normal tasks so genuinely small tasks aren't blocked.

## 3. Hard-task escalation

Where `RigorPolicy` bites, beyond gates 2.5/2.6:

- `Verifier.verify_task()`: when task is hard and `enforce_coverage_on_hard`, use the existing `_diff_coverage_override` hook to set enforce=True for that run (turns `diff_not_exercised` blocking).
- `PromptBuilder.build_task_prompt()`: inject a compact **Rigor** section for hard tasks (~15 lines): this task is classified hard and verification is strict; no stubs/TODOs — blocking; every AC needs a passing behavioral check; changed lines must be exercised by tests; don't claim completion without running the expected tests; if genuinely infeasible, say so explicitly and state what's missing instead of stubbing.
- `dev go` repair loop: `_max_repair_attempts` += `extra_repair_attempts_on_hard` for hard tasks.
- (Opt-in) `reviewer_required_on_hard`: run `ImplementationReviewer` on hard tasks during verify; "Critical Issues" → blocking `architecture_drift` gap (existing plumbing).

Difficulty + policy recorded in `VerificationOutcome` (new fields `difficulty`, `rigor_applied: list[str]`) so "passed" vs "passed under strict gates" is auditable.

## 4. Repair-loop prompting

`CorrectionManifest` additions (backward-compatible, defaulted):

- `attempt_history: list[str]` — one line per prior attempt: root cause it targeted + whether gaps changed ("attempt 1: targeted test_failed on X; blocked again with identical gaps"). Sourced from persisted manifest records.
- `approach_guidance: str` — deterministic escalation text: attempt ≥2 with identical gaps → "Your previous approach failed the same way. Do NOT retry the same edit; re-read the failing output and change strategy."; final budgeted attempt → "Final attempt. If the criterion cannot be met, leave a clear analysis in your response instead of stubbing or weakening tests."
- `stub_findings: list[str]` — `file:line reason` from Gate 2.5, so repair prompts name the exact placeholders to replace.
- Anti-fake rules appended wherever the manifest is rendered into an executor prompt (coding_cli / claude_sdk / native agent): completion may only be claimed after `commands_to_rerun` pass locally; never delete or skip tests to pass; never weaken an assertion to pass.

## Files touched

| File | Change |
|---|---|
| `verification/difficulty.py` (new) | estimator + RigorPolicy |
| `verification/stub_detector.py` (new) | Gate 2.5 |
| `verification/verifier.py` | call gates 2.5/2.6, coverage override on hard, outcome fields |
| `domain/gap.py` | +`stub_detected`, +`suspicious_effort` |
| `domain/task.py` | +optional `difficulty` |
| `app/config.py` | +`RigorConfig` under `verification` |
| `verification/next_actions.py` | category/action for new gap types |
| `planning/correction_manifest.py` | attempt_history, approach_guidance, stub_findings |
| `execution/prompt_builder.py` | hard-task Rigor section |
| `cli/commands/go.py` | extra repair attempts on hard |
| executor manifest rendering | anti-fake rules |
| `tests/unit/test_difficulty.py`, `test_stub_detector.py`, `test_effort_heuristics.py` (new) + extend `test_correction_manifest.py` | coverage |

## Non-goals / risks

- No LLM in difficulty estimation or stub detection — deterministic and cheap; the existing LLM reviewer already handles the fuzzy layer.
- False positives contained: added-lines-only scanning, allow-stub escape hatch, advisory-by-default outside hard tasks, conservative effort thresholds.
- Gap Literal is closed — every new gap type is threaded through next_actions and manifest priority in the same change to keep the repair contract consistent.
