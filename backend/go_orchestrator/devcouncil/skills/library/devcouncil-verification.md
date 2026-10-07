---
name: devcouncil-verification
title: DevCouncil Verification & Next Actions
description: Interpret DevCouncil verification results — blocking gaps, typed next_actions, diff coverage, and the stub and secret gates — and repair until evidence proves the diff.
triggers:
  keywords: [verification, verify, blocking gap, next action, diff coverage, dev verify, acceptance evidence, rigor]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Verification & Next Actions

DevCouncil's verifier is **deterministic** (no extra LLM calls for stub or secret detection).
It decides pass/fail from evidence: diffs, test output, coverage intersection, and policy checks.

This skill does not make verification mandatory. Check `gates.mode` first
(`dev gate status`):

- `enforce`: verification and effective blocking gaps govern task release.
- `advisory`: verification is optional; quality findings are recorded as advisory.
- `off`: quality verification is skipped. Report work as completed unverified.

Hard-safety findings remain blocking in every mode.

## Verification outputs

`devcouncil_verify_task` (or `dev verify TASK-ID --json`) returns:

- **`passed`** — no blocking gaps remain
- **`blocking_gaps`** — must fix before release only in `enforce` (hard safety
  remains blocking in every mode)
- **`next_actions`** — typed, machine-routable repair instructions
- **`rigor_applied`** — which rigor gates ran; when none did,
  **`rigor_skipped_reason`** says why. An empty list is not a clean result.

Read persisted state without re-verifying:

```
devcouncil_get_gaps           # gap list (blocking_only filter available)
devcouncil_get_diff           # working-tree diff, optionally scoped to the task
```

Those two are the whole persisted-state surface this host serves. The typed
actions and `allowed_next_tools` ride on the `devcouncil_verify_task` result;
evidence and the audit trail come from `dev verify TASK-ID --json` and the
persisted state under `.devcouncil/`.

## Next-actions contract

Each action includes:

| Field | Use |
|---|---|
| `gap_id` | Stable identifier for tracking |
| `gap_type` | e.g. `diff_not_exercised`, `stub_detected`, `orphan_diff` |
| `category` | Branch on this: `fix_code`, `add_test`, `fix_verification`, `scope`, `security`, `review`, `plan`, `refresh_map` |
| `severity` | `high` / `medium` / `low` |
| `blocking` | Must fix when true |
| `action` | Human-readable fix instruction |
| `file` / `line` | Precise location |
| `suggested_command` | Command to run after fixing |

Act on **blocking** actions first. Advisory actions surface quality issues but do not
block release.

## Rigor gates

The stub, secret and diff↔coverage gates run in the `dcverify` binary, which
verification spawns whenever it is installed. What blocks is fixed by the gate, not
by configuration or task difficulty:

| Finding | Gap type | Blocks? |
|---|---|---|
| Added code whose body is a placeholder (`todo!()`, `unimplemented!()`, `raise NotImplementedError`) | `stub_detected` | Yes |
| Added function with an empty body and a comment inside saying it is unfinished | `stub_detected` | Yes |
| Added function with an empty body and no such comment | `stub_detected` (low) | No |
| Added `TODO` / `FIXME` / placeholder marker in a comment or string | `stub_detected` (low) | No |
| Added test that is unconditionally skipped | `skipped_test` | No |
| Added test that asserts nothing | `assert_free_test` | No |
| A stub covered by an `allow-stub: <reason>` marker | `stub_declared` | No |
| Added line shaped like a credential (vendor token, private key block) | `security_risk` | Yes |
| Changed lines no test executed | `diff_not_exercised` | No |
| Diff that is mostly relocation, repetition or generated output | `low_substance` | No |
| `dcverify` configured but failed to answer | `rigor_check_unavailable` | Yes |

The blocking rows are hard-safety gap types (`devcouncil/gating/policy.go`), so
`advisory` does not demote them. An `allow-stub` marker goes on the stub's line or
above its function and must give a reason (`// allow-stub: waiting on the v2 API`);
a bare marker is ignored, and no marker covers a credential. Where `dcverify` can
parse the file, stubs and tests are judged from its syntax tree; elsewhere it
falls back to matching lines.

**Diff↔coverage** only counts a passing suite if it **executed the changed lines**, by
intersecting a coverage profile with the diff hunks. It runs only when a profile is
supplied — `dev verify TASK-ID --coverage PATH`; the MCP tool supplies none — and the
`coverage_skipped_reason` field says when it did not run. Its gap is always advisory:
no setting promotes it to blocking.

A failed verification also writes a correction manifest — the gaps and their next
actions — beside the task. Repair from it without weakening tests or stubbing around
a gap.

## Repair workflow (`enforce`, or when explicitly requested)

1. Read `next_actions` from the `devcouncil_verify_task` result, or
   `devcouncil_get_gaps` for the persisted list — list blocking items
2. Fix each gap (smallest change that closes it)
3. Re-run the suggested tests with your host's own command tool
4. `devcouncil_verify_task` — repeat until `passed`

A gap that needs a file outside the task's planned scope is a question for the task's
owner; no tool widens planned files.

## CLI equivalent

```bash
dev verify TASK-ID --json                   # full task verification
dev verify TASK-ID --json --coverage PATH   # with diff↔coverage from a profile
```

## When to call success

- In `enforce`, only claim verified success when `passed: true` with zero
  effective blocking gaps.
- In `advisory`, distinguish test results from optional DevCouncil verification
  and report advisory findings honestly.
- In `off`, completion does not require a task or verifier call; say
  **completed unverified** and include any tests you chose to run.
