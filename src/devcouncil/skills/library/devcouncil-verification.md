---
name: devcouncil-verification
title: DevCouncil Verification & Next Actions
description: Interpret DevCouncil verification results — blocking gaps, typed next_actions, diff coverage, and anti-laziness rigor — and repair until evidence proves the diff.
triggers:
  keywords: [verification, verify, blocking gap, next action, diff coverage, dev check, dev verify, dev gaps, acceptance evidence, rigor]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Verification & Next Actions

DevCouncil's verifier is **deterministic** (no extra LLM calls for stub/effort detection).
It decides pass/fail from evidence: diffs, test output, coverage intersection, and policy checks.

## Verification outputs

`devcouncil_verify_task` (or `dev verify`, `dev check --verify`) returns:

- **`passed`** — no blocking gaps remain
- **`blocking_gaps`** — must fix before release
- **`next_actions`** — typed, machine-routable repair instructions

Read persisted state without re-verifying:

```
devcouncil_get_gaps           # gap list (blocking_only filter available)
devcouncil_get_next_actions   # blocking + advisory actions, allowed_next_tools
devcouncil_get_evidence       # command results, test evidence
devcouncil_get_task_provenance  # audit trail: writes, verify runs, diff coverage
```

## Next-actions contract

Each action includes:

| Field | Use |
|---|---|
| `gap_id` | Stable identifier for tracking |
| `gap_type` | e.g. `diff_not_exercised`, `stub_detected`, `orphan_diff` |
| `category` | Branch on this: `fix_code`, `add_test`, `fix_verification`, `scope`, `security`, `review`, `plan` |
| `severity` | `high` / `medium` / `low` |
| `blocking` | Must fix when true |
| `action` | Human-readable fix instruction |
| `file` / `line` | Precise location |
| `suggested_command` | Command to run after fixing |

Act on **blocking** actions first. Advisory actions surface quality issues but do not
block release unless configured to.

## Diff↔coverage gate

A passing test suite only counts if it **executed the changed lines**. The verifier
intersects coverage data with diff hunks. An unexercised diff yields `diff_not_exercised`.

- **Signal-first by default** — informational unless `verification.diff_coverage.enforce: true`
- **Hard tasks** — `verification.rigor.enforce_coverage_on_hard` promotes to blocking
- **Degrades silently** when coverage tooling or parseable diff is unavailable — never
  blocks correct work for lack of measurement

Config (`.devcouncil/config.yaml`):

```yaml
verification:
  diff_coverage:
    measure: true
    enforce: false
    min_ratio: 0.0
```

## Anti-laziness rigor

Scales strictness by task **difficulty** (`easy` / `normal` / `hard`):

| Gate | Easy/Normal | Hard |
|---|---|---|
| Stub/TODO detection | Advisory | Blocking |
| Effort heuristics (undersized diff) | Advisory | Blocking |
| Coarse acceptance proof | Advisory | Blocking |
| Diff coverage | Advisory (unless enforce) | Blocking (default) |

Stub markers (`TODO`, `NotImplementedError`, assert-free tests) block on hard tasks unless
the task mentions scaffolding and the line carries `devcouncil: allow-stub`.

Repair runs include a **correction manifest** with prior diff, failing output, and
non-negotiable rules: never weaken tests, never stub around a gap.

## Repair workflow

1. `devcouncil_get_next_actions` — list blocking items
2. Fix each gap (smallest change that closes it)
3. Re-run suggested tests via `devcouncil_run_command`
4. `devcouncil_verify_task` — repeat until `passed`
5. `/devcouncil:repair [TASK-ID]` or `dev repair [TASK-ID]` for CLI-guided repair

## CLI equivalents

```bash
dev verify [TASK-ID]          # full task verification
dev gaps [TASK-ID]            # list gaps
dev check --verify --test "…" # inline requirement on current diff
dev report                    # Requirement→Task→Diff→Evidence coverage
dev report rigor              # tune rigor thresholds from evidence
```

## When to call success

Only when **`passed: true`** from `devcouncil_verify_task` with zero blocking gaps.
Report the verification result and cite `file:line` for any remaining advisory findings.
Never claim done based on test output alone without running verify.
