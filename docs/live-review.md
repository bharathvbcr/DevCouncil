# Live Coding-Agent Review

DevCouncil includes a Sage-style watch surface while keeping the evidence gate as the final authority.

```bash
dev watch sessions --client claude
dev watch review --client claude --latest
dev watch review --client claude --session SESSION-ID
dev watch review --client claude --transcript path/to/claude-session.jsonl --task-id TASK-001
dev watch review --client claude --transcript path/to/claude-session.jsonl --llm
dev watch follow --client claude --latest
dev watch signals --client claude
dev watch pending --client claude
dev watch pending --client claude --task-id TASK-001
dev watch status --task-id TASK-001
dev watch cards
dev watch cards --task-id TASK-001 --status open --verdict critical
dev watch repair CARD-ID
dev watch repair-all --task-id TASK-001
dev watch resolve CARD-ID --status resolved
```

`dev watch review` normalizes Claude-style or generic JSONL transcripts, reviews the latest assistant response, and writes a critique card under `.devcouncil/live/cards/`. Each card includes:

- Verdict: `Approved`, `Concerns`, or `Critical Issues`.
- Concerns and safer alternatives.
- Evidence requests when an agent claims completion without proof.
- A ready-to-paste message for the coding agent.

By default, live review uses a deterministic local reviewer so it works without API keys. Add `--llm` to use the configured `live_reviewer` role, falling back to `implementation_reviewer` when needed. If model setup is unavailable, DevCouncil prints the reason and emits the deterministic card instead.

`dev watch review --latest` reviews the most recently updated discovered session. Use `--session SESSION-ID` to select a specific row from `dev watch sessions`, or `--transcript` when you already know the JSONL path.

`dev watch follow` polls the selected transcript and emits a new card whenever the latest assistant turn changes. This gives DevCouncil the same sidecar shape as Sage while keeping the card artifacts local and tied to DevCouncil's evidence workflow.

Claude hook setup also installs a response-ready signal hook. When Claude finishes a turn, DevCouncil records a signal under `.devcouncil/live/signals/` so the session can be reviewed without losing the thread.

Use `dev watch signals` to inspect pending hook signals. Use `dev watch pending` to review every signal that includes a transcript path; successfully reviewed signals are moved to `.devcouncil/live/signals/processed/` so cards are not duplicated. Scoped signals include `--task-id` in their replay command, and `dev watch pending --task-id TASK-001` can force a scope when consuming pending signals.

Cards start as `open`. Open `Critical Issues` cards block `dev verify` as live-review gaps until they are addressed with `dev watch resolve CARD-ID --status resolved` or intentionally dismissed with `--status ignored`.

Use `dev watch repair CARD-ID` to generate a ready-to-paste correction prompt grounded in the card and, when available, the original DevCouncil task contract. Use `dev watch repair-all --task-id TASK-001` to generate one combined repair prompt for all blocking cards in scope.

Use `dev watch status` for a compact summary of pending signals, copyable pending review commands, card counts, open critical cards, current task scope, and blocking cards for that scope.

Use `dev watch cards` to inspect stored critique cards. It supports `--task-id`, `--status`, `--verdict`, and `--client` filters for narrowing a growing card history.

The normal `dev status --json` payload also includes a `live_review` section so dashboards and scripts can see the same card/signal state without calling the watch subcommand.

`dev report`, `dev report --json`, and PR/MR comment reports include the same live-review summary. Open critical live-review blockers change the report verdict to `blocked`.

Live-review card saves, reused cards, processed response signals, and status updates are also written to the DevCouncil trace stream, so `dev trace tail` can audit the critique-card lifecycle.

MCP clients can read the same state through the `devcouncil_live_review` tool, list filtered critique cards through `devcouncil_live_cards`, request a single-card remediation prompt through `devcouncil_live_repair_prompt`, or request a scoped bulk prompt through `devcouncil_live_repair_all`. The `devcouncil_report` MCP tool also includes the live-review section.

Reviews are idempotent by default: the same session turn maps to the same card ID, and rerunning `dev watch review` or `dev watch pending` will reuse the existing card instead of reopening resolved work. Use `--force` to regenerate the card while preserving its current lifecycle status.

Use `--task-id TASK-001` when reviewing or following a session for a specific DevCouncil task. Verification blocks open critical cards for the task being verified plus unscoped critical cards, but it does not block on cards explicitly scoped to a different task.

When exactly one DevCouncil task is `running`, `dev watch review`, `dev watch follow`, and `dev hook agent-response` automatically attach that task ID. If no task or multiple tasks are running, DevCouncil leaves the card unscoped unless `--task-id` is provided.

## Difference From Sage

**Sage** reviews an active coding-agent session and provides critique cards to help the developer course-correct.

**DevCouncil** does that at the DevCouncil boundary and also focuses on gated execution:

- It creates a persistent requirement, task, diff, and evidence graph.
- It blocks task completion when required evidence is missing.
- It detects orphan diffs and unauthorized architectural changes.
- It produces a deterministic evidence report for the final implementation.
- It emits local critique cards for coding-agent responses through `dev watch`.

Sage asks: "Is this agent response good?" DevCouncil asks: "Can this task prove it satisfied the requirement?"
