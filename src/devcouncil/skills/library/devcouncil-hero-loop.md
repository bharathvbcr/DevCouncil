---
name: devcouncil-hero-loop
title: DevCouncil Hero Loop (Checkout → Verify → Release)
description: Run DevCouncil's autonomous task loop in Claude Code — checkout a lease, implement inside scope, verify with deterministic gates, self-repair from typed next_actions, then release.
triggers:
  keywords: [hero loop, verify loop, checkout task, verify task, release task, next task, dev go, dev e2e, dev repair, task lease]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Hero Loop

DevCouncil's certified end-to-end path for Claude Code: the agent checks out a task,
implements inside declared scope, verifies deterministically, self-repairs from typed
next actions, and releases — **without a human pasting test output back and forth.**

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
devcouncil_next_task          # highest-priority unblocked task (or use a known TASK-ID)
devcouncil_checkout_task      # acquire lease — required before writes or verify
devcouncil_get_task           # scope, planned files, acceptance criteria
devcouncil_get_prompt           # full executor prompt with rigor + context
```

One agent owns the task at a time. If checkout fails (lease held), do not bypass — pick
another task or wait for release.

### 2. Implement inside scope

- Read with `devcouncil_read_file`; inspect changes with `devcouncil_get_diff`.
- **Write only through the gate:** `devcouncil_write_file` / `devcouncil_apply_patch`.
  Direct editor writes to out-of-scope or protected paths are rejected.
- Run tests with `devcouncil_run_command` or `devcouncil_record_command`.
- Preflight questionable paths with `devcouncil_policy_check_write`.

Stay inside the task's **planned files** and **allowed commands**. Do not expand scope
silently — use `devcouncil_update_task_scope` only when the task genuinely requires it.

### 3. Verify

```
devcouncil_verify_task        # deterministic verifier — requires active lease
```

Returns `passed`, `blocking_gaps`, and `next_actions`. A green test suite is not enough;
the verifier checks planned-file compliance, orphan diffs, acceptance evidence, diff
coverage, stub detection, and more.

### 4. Self-repair from next_actions

When `passed` is false:

```
devcouncil_get_next_actions   # cheap read of persisted gaps — no re-verify
```

Each action is typed (`category`: `fix_code`, `add_test`, `fix_verification`, `scope`,
`security`, `review`, `plan`) with `file`, `line`, and often `suggested_command`.
Fix each blocking action, then call `devcouncil_verify_task` again. Repeat until clean.

Do **not** weaken tests, stub around gaps, or skip verification to force a pass.

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

- Editing files before checkout (writes may fail or lack provenance).
- Calling `devcouncil_verify_task` without a lease (rejected).
- Declaring done on test pass alone (verifier may report `diff_not_exercised`, stubs, scope gaps).
- Ignoring `next_actions` categories — branch on `category`, not prose parsing.
