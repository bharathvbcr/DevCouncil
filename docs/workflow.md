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

`dev e2e` and `dev go` share the same end-to-end implementation: plan, execute, verify, report. If `--executor` is omitted, they use `execution.default_executor` from `.devcouncil/config.yaml`.

**One-command onboarding:** `dev boot "goal"` runs setup, applies integrations (unless `--skip-integrations`), optionally scaffolds CI, then hands off to `dev go`. See [quickstart.md](quickstart.md).

For machine-readable integration, add `--agent` with the selected automated executor. It enables JSON report output and writes `.devcouncil/reports/latest.json`.

## 1. Create The Implementation Plan

```bash
dev plan "Add password reset with expiring single-use tokens"
```

DevCouncil maps the repository, drafts requirements, runs planner and critic roles, and stores an approved task graph locally. To preview the interactive code-graph UI before mapping, run `dev graph demo` (writes `.devcouncil/graph/demo.html` with a synthetic import graph).

Before planning (or after large refactors), refresh navigation artifacts and preview the graph UI:

```bash
dev map                 # repo_map.json + code_graph.json + AGENTS.md/CLAUDE.md
dev graph ingest        # unified analyze entry (alias path: sync + map + optional embeddings)
dev graph query SYMBOL  # callers / callees
dev graph dead          # dead-code tiers
dev graph demo          # sample visualizer UI (no map) + SVG preview
dev graph view          # serve interactive graph.html for this repo
dev corpus build        # advisory docs/PDF/image index (optional verify gates)
```

A missing or stale map fails closed on hard rigor — run `dev map` or `dev graph ingest`
before `dev verify` on strict tasks. Write policy soft-blocks edits outside planned
files unless the target is in the same subsystem or a map neighbor (`dev scope update`
to widen scope).

See [code-graph.md](code-graph.md) for the full map/graph surface and [corpus.md](corpus.md)
for the documentation side index.

Inspect the plan:

```bash
dev status
dev tasks
dev show TASK-001
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
- With Claude/Codex hooks installed, Stop runs claim checks + optional task verify (`execution.stop_gate`); treat those messages as completion evidence, not just chat noise.

## 7. Live review (`dev watch`)

Optional Sage-style sidecar while verification remains the final authority:

```bash
dev watch sessions --client claude
dev watch review --client claude --latest
dev watch follow --client claude --latest
dev watch pending --client claude
dev watch cards
dev watch status --task-id TASK-001
```

`dev watch review` normalizes Claude-style or generic JSONL transcripts, writes critique cards under `.devcouncil/live/cards/`, and can block `dev verify` on open `Critical Issues` until `dev watch resolve CARD-ID`. Deterministic local reviewer by default; add `--llm` for the configured live-reviewer role. MCP: `devcouncil_live_review`, `devcouncil_live_cards`, `devcouncil_live_repair_prompt`.
