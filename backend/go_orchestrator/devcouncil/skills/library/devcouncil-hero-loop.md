---
name: devcouncil-hero-loop
title: DevCouncil Hero Loop (Checkout → Verify → Release)
description: Run DevCouncil's autonomous task loop in Claude Code — checkout a lease, implement inside scope, verify with deterministic gates, self-repair from typed next_actions, then release.
triggers:
  keywords: [hero loop, verify loop, checkout task, verify task, release task, next task, dev go, dev e2e, dev repair, task lease]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Hero Loop

This is an **opt-in strict task workflow**. It does not override
`gates.mode=off` or make tasks, leases, or verification mandatory for ordinary
work. Run it only when the user explicitly requests the hero/task loop or when
strict enforcement is active.

DevCouncil's certified end-to-end path for Claude Code uses the **verify, lease, and MCP modules**: the agent checks out a task,
implements inside declared scope, verifies deterministically, self-repairs from typed
next actions, and releases — **without a human pasting test output back and forth.**
Manvi wraps the same components for multi-agent campaigns; GitPulse uses Manvi and selected DevCouncil modules rather than the full suite.

## The loop

```
checkout_task ─▶ (agent implements) ─▶ verify_task ─▶ passed? ─▶ release_task
      ▲                                     │
      │                                     ▼ blocking gaps
      └──────────── self-repair ◀──── next_actions (typed)
```

## Step-by-step workflow

### 1. Pick up work

```
devcouncil_next_task          # highest-priority unblocked task (or a known TASK-ID)
devcouncil_checkout_task      # acquire lease — required before writes or verify when
                              # write-gates / contain mode are active
```

Checkout returns the scope with the lease: planned files, allowed commands,
expected tests and gate mode all ride on its result. `devcouncil_get_task` and
`devcouncil_get_prompt` belonged to the retired Python host and are not served.

One agent owns the task at a time. If checkout fails (lease held), do not bypass — pick
another task or wait for release.

### 2. Implement inside scope

- Read and edit files, and run tests, with your host's own tools. This host serves
  no `devcouncil_read_file`, `devcouncil_write_file`, `devcouncil_apply_patch`,
  `devcouncil_run_command` or `devcouncil_record_command` — they belonged to the
  retired Python host, and nothing gates a write as it happens.
- Inspect changes with `devcouncil_get_diff`.
- Preflight questionable paths with `devcouncil_policy_check_write`, the one policy
  tool that survived. It answers the scope question; it does not perform the write.

Stay inside the task's **planned files** and **allowed commands**. Do not expand scope
silently: there is no `devcouncil_update_task_scope` here, so a task that genuinely
needs a wider scope is a question for the human, not a tool call.

When the advisor tool is available, consult it before committing to an approach, when
stuck on a recurring error, and before declaring the task complete.

### 3. Verify

```
devcouncil_verify_task        # deterministic verifier — lease required in enforce
```

Returns `passed`, `blocking_gaps`, and `next_actions`. A green test suite is not enough;
the verifier checks planned-file compliance, orphan diffs, acceptance evidence, diff
coverage, stub detection, and more.

### 4. Self-repair from next_actions

When `passed` is false, the typed actions are already in the verify result. To
re-read the persisted gaps later without verifying again:

```
devcouncil_get_gaps           # cheap read of persisted gaps — no re-verify
```

Each action is typed (`category`: `fix_code`, `add_test`, `fix_verification`, `scope`,
`security`, `review`, `plan`) with `file`, `line`, and often `suggested_command`.
Fix each blocking action, then call `devcouncil_verify_task` again. Repeat until clean.

Inside this opted-in loop, do **not** weaken tests, stub around gaps, or skip
verification to force a pass.

### 5. Release

```
devcouncil_release_task       # only after verify passes (or explicit abandon policy)
```

Report: task ID, what changed, verification result, and any advisory (non-blocking) gaps.

## Alternatives to manual MCP driving

| Entry | When |
|---|---|
| `/devcouncil:next` | Interactive Claude Code — shells out then instructs MCP loop |
| `dev go [TASK-ID]` | CLI-driven loop with repair budget |
| `dev e2e "<goal>" --executor claude` | Full plan → implement → verify automation |
| Subagent `devcouncil-implementer` | Delegate the entire loop |

## Lite on-ramp (no task graph yet)

Before full planning, taste the evidence gate on the current diff:

```bash
dev check --verify --goal "requirement text" --test "python -m pytest tests/ -q"
```

Same deterministic verifier as the hero loop. Graduate to `dev plan` once you trust the gate.

## Common mistakes

- Skipping checkout during the **hero loop** / contain mode (MCP gated writes and verify need a lease).
  Interactive assist Shell does not require checkout.
- Calling `devcouncil_verify_task` without a lease in `enforce` mode (rejected).
- Declaring done on test pass alone (verifier may report `diff_not_exercised`, stubs, scope gaps).
- Ignoring `next_actions` categories — branch on `category`, not prose parsing.
