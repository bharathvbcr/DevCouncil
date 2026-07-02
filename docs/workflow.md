# Daily Workflow

DevCouncil's recommended default is **Manual Sidecar Mode**:

1. DevCouncil plans the work and creates a task graph.
2. You ask DevCouncil for one constrained task prompt.
3. You paste that prompt into your coding CLI or agent.
4. The agent edits the repository.
5. DevCouncil verifies the resulting diff against task constraints.
6. If verification fails, DevCouncil creates a focused repair loop.

For coding agents that need one entrypoint instead of the task-by-task sidecar loop, run:

```bash
dev e2e "Describe the implementation goal" --executor codex
```

`dev e2e` and `dev go` share the same end-to-end implementation: plan, execute, verify, report. If `--executor` is omitted, they use `execution.default_executor` from `.devcouncil/config.yaml`. When planning leaves advisory gaps, `dev e2e` stops until you run `dev approve` or re-run with `--force`.

For machine-readable integration, add `--agent` with the selected automated executor. It enables JSON report output and writes `.devcouncil/reports/latest.json`.

## 1. Create The Implementation Plan

```bash
dev plan "Add password reset with expiring single-use tokens"
```

DevCouncil maps the repository, drafts requirements, runs planner and critic roles, and stores the task graph locally. If all plan gates pass, the project moves to `PLAN_APPROVED` automatically. When advisory gaps remain, the phase stays `AWAITING_USER_DECISIONS` until you approve the plan.

Inspect the plan:

```bash
dev status
dev tasks
dev show TASK-001
```

If `dev status` shows `AWAITING_USER_DECISIONS`, approve before running tasks:

```bash
dev approve              # accept the generated plan
dev approve --force      # approve even when blocking gate gaps remain
```

## 2. Start One Task

```bash
dev run TASK-001 --executor manual
```

This creates a checkpoint and marks the task as running. DevCouncil expects the next repository diff to match this task's allowed files, acceptance criteria, and verification commands.

## 3. Generate The Coding Prompt

```bash
dev prompt TASK-001
```

Paste the full output into your coding CLI. The generated prompt includes the task objective, allowed files, constraints, acceptance criteria, and evidence requirements.

## 4. Verify The Result

After the coding CLI modifies the repository:

```bash
dev verify TASK-001
```

Verification records evidence and marks the task as either `verified` or `blocked`.

For a quick deterministic gate without a planned task (no provider keys):

```bash
dev check --verify --goal "reset tokens are single-use" --test "pytest tests/test_auth.py -q"
```

Inspect the result:

```bash
dev status
dev report
dev report --json
```

## 5. Repair Gaps

If verification blocks the task, convert the gaps into focused repair work:

```bash
dev repair
dev tasks
dev prompt REPAIR-001
```

Paste the repair prompt into the same coding CLI, then verify again:

```bash
dev verify REPAIR-001
dev verify TASK-001
```

## 6. Continue Task By Task

```bash
dev tasks
dev show TASK-002
dev run TASK-002 --executor manual
dev prompt TASK-002
dev verify TASK-002
dev report
```

Recommended working rules:

- Run DevCouncil and the coding CLI from the same repository root.
- Give the coding CLI one DevCouncil task prompt at a time.
- Do not ask the coding CLI to broaden scope beyond the generated prompt.
- Run `dev verify TASK-ID` before committing agent-generated changes.
- Use `dev repair` for follow-up fixes instead of free-form retry prompts.
- Use `dev rollback TASK-ID` if a task needs to be reverted from its checkpoint.
- Treat `.devcouncil/` as local project state and the audit trail for the gated run.
