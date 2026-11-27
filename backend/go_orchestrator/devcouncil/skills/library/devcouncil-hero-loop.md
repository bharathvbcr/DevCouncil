---
name: devcouncil-hero-loop
title: DevCouncil Hero Loop (Checkout → Verify → Release)
description: Run DevCouncil's autonomous task loop in Claude Code — checkout a lease, implement inside scope, verify with deterministic gates, self-repair from typed next_actions, then release.
triggers:
  keywords: [hero loop, verify loop, checkout task, verify task, release task, next task, task lease]
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
Manvi wraps the same components for multi-agent campaigns; Waylode (formerly GitPulse) uses Manvi and selected DevCouncil modules rather than the full suite.

## The loop

```
checkout_task ─▶ (agent implements) ─▶ verify_task ─▶ passed? ─▶ release_task
      ▲                                     │
      │                                     ▼ blocking gaps
      └──────────── self-repair ◀──── next_actions (typed)
```

## Step-by-step workflow

Each step below names the MCP tool; from a shell run the same tool as
`dev tool call <name> --args '<json>'` (for example
`dev tool call devcouncil_checkout_task --args '{"task_id":"T-1","client_id":"me"}'`).
Reuse one `client_id` across MCP and shell so the lease is yours in both.
Every `devcouncil_*` tool also accepts optional `repo_path` (alias `root`) when
talking to a globally registered `devcouncil mcp` server across multiple repositories.

### 1. Pick up work

```
devcouncil_next_task          # highest-priority unblocked task (or a known TASK-ID)
devcouncil_checkout_task      # acquire lease — required before verify in enforce
```

Checkout returns the scope with the lease: planned files, allowed commands,
expected tests and gate mode all ride on its result.

One agent owns the task at a time. If checkout fails (lease held), do not bypass — pick
another task or wait for release.

### 2. Implement inside scope

- Read and edit files, and run tests, with your host's own tools. DevCouncil serves
  no file or shell tools, and nothing gates a write as it happens.
- Inspect changes with `devcouncil_get_diff`.
- Preflight questionable paths with `devcouncil_policy_check_write`, the one policy
  tool that survived. It answers the scope question; it does not perform the write.
  Pass your `task_id` and the `operation` (`create`, `modify` or `delete`): without
  `task_id` the answer stops at `task.absent` and says nothing about scope.

Stay inside the task's **planned files** and **allowed commands**. Do not expand scope
silently: no tool widens a task's planned files, so a task that genuinely needs a
wider scope is a question for its owner, not a tool call.

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
`security`, `review`, `plan`, `refresh_map`) with `file`, `line`, and often `suggested_command`.
Fix each blocking action, then call `devcouncil_verify_task` again. Repeat until clean.

Inside this opted-in loop, do **not** weaken tests, stub around gaps, or skip
verification to force a pass.

### 5. Release

```
devcouncil_release_task       # only after verify passes (or explicit abandon policy)
```

Report: task ID, what changed, verification result, and any advisory (non-blocking) gaps.

## Waylode multi-agent handoffs (worktrees, A2A, landing)

A DevCouncil lease makes the task yours; it does not isolate your working tree or
prevent another worktree from editing the same files. When **Waylode** (formerly
GitPulse) MCP tools are available, pair the hero loop with Waylode across three seams:

1. **Before editing (isolation & collision preflight):**
   - Call `waylode_insights` to see active worktrees, agent sessions, and contended files,
     and `waylode_collision_risk` on your task's planned paths before touching them.
   - Provision an isolated lane worktree with `waylode_worktree_create` (with
     `share_cache: true`) so edits stay isolated from sibling lanes.
2. **During execution (A2A peer coordination):**
   - Register your lane with `waylode_agent_register` and discover peers via
     `waylode_agent_peers`.
   - Coordinate overlapping interfaces or handoffs with `waylode_agent_send`,
     `waylode_agent_peek` / `waylode_agent_inbox`, `waylode_agent_wait`, or structured
     asks (`waylode_inbox_post`, `waylode_inbox_read`, `waylode_inbox_answer`).
3. **After `devcouncil_verify_task` passes & `devcouncil_release_task` completes (landing & wrap-up):**
   - Commit cleanly inside your lane (`waylode_worktree_commit`), then hand off to
     Waylode's **commit-and-merge** workflow (`waylode_worktree_merge` or
     `waylode_worktree_merge_batch`) to land onto the integration branch safely.
   - Finish with Waylode's **wrap-up** hygiene: drain remaining inbox messages,
     `waylode_agent_deregister`, and prune integrated worktrees with
     `waylode_worktree_prune`.

(Sessions started before the rename list these tools as `gitpulse_*`; legacy aliases
remain accepted, and an unavailable Waylode call must be reported as unestablished,
never as clean.)

## Alternatives to manual MCP driving

| Entry | When |
|---|---|
| Subagent `devcouncil-implementer` | Delegate the entire loop |
| `dev tool call <name> --args '<json>'` | Lead with this from a shell or a Code Mode REPL script (see the **devcouncil** skill): same registry, answer text and lease as MCP, which stays the alternative. Exit 0 answered, 1 refused, 2 malformed, 3 could not run here. Say that you used the shell when MCP was unreliable |
| `dev verify TASK-ID --json` | Scripting or CI: the same verifier, from a shell |

DevCouncil has no command that plans a goal into tasks or drives the loop for you;
Manvi is the harness that runs it end to end.

## Common mistakes

- Skipping checkout during the **hero loop** (verification needs a lease in `enforce`).
  Interactive assist Shell does not require checkout.
- Calling `devcouncil_verify_task` without a lease in `enforce` mode (rejected).
- Declaring done on test pass alone (verifier may report `diff_not_exercised`, stubs, scope gaps).
- Ignoring `next_actions` categories — branch on `category`, not prose parsing.
