---
name: devcouncil
title: DevCouncil Integration for Claude Code
description: Operate inside a DevCouncil-managed repo — use MCP tools and slash commands for status, scope, policy-gated writes, and evidence-first verification instead of guessing project state.
triggers:
  keywords: [devcouncil, dev council, devcouncil mcp, mcp devcouncil, dev integrate]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Integration for Claude Code

This repository is managed by **DevCouncil**: a planning, execution, and verification
layer for coding agents. Evidence — not model confidence — decides when work is done.

Use DevCouncil's MCP tools and `dev` CLI for project state, task scope, and verification.
Do not guess at task status, file scope, or whether tests actually prove the diff.

## Navigate before you edit

1. Open `.devcouncil/repo_map.json` for subsystem entry points, critical files, and
   cross-subsystem handoff paths. Regenerate with `dev map` after large refactors.
2. Read `AGENTS.md` / `CLAUDE.md` for workspace conventions.
3. Check status: `devcouncil_status` (MCP) or `/devcouncil:status` (slash command) or
   `dev status` (CLI).

## MCP tools vs CLI vs slash commands

| Need | Prefer |
|---|---|
| Task loop (checkout, write, verify, release) | MCP tools (`mcp__devcouncil__devcouncil_*`) |
| Quick human-readable status / reports | Slash commands (`/devcouncil:status`, `/devcouncil:report`) |
| Scripting, CI, headless runs | `dev` CLI (`dev verify`, `dev go`, `dev e2e`) |
| Read-only inspection (gaps, provenance, diff) | MCP read tools before re-running verify |

MCP tool names in Claude Code are prefixed: `mcp__devcouncil__devcouncil_<name>`.

## Slash commands (Claude Code)

Install with `dev integrate claude --apply`. Commands live under `/devcouncil:*`:

| Command | Purpose |
|---|---|
| `/devcouncil:status` | Phase, tasks, blocking gaps |
| `/devcouncil:next` | Pick up the next unblocked task via MCP |
| `/devcouncil:verify [TASK-ID]` | Run verification, report gaps |
| `/devcouncil:repair [TASK-ID]` | Repair blocking gaps |
| `/devcouncil:plan <goal>` | Plan a goal into requirements/tasks |
| `/devcouncil:review [TASK-ID]` | Live-review critique cards |
| `/devcouncil:report` | Full coverage report |
| `/devcouncil:map [goal]` | Refresh/read the repo map |
| `/devcouncil:wiki [topic]` | Consult the codebase wiki |
| `/devcouncil:supervise [RUN-ID]` | Review a recorded agent run |

## Key MCP tools (by role)

**Orientation:** `devcouncil_status`, `devcouncil_report`, `devcouncil_integration_status`,
`devcouncil_wiki_page`, `devcouncil_graph_context`

**Task loop:** `devcouncil_next_task`, `devcouncil_checkout_task`, `devcouncil_get_task`,
`devcouncil_get_prompt`, `devcouncil_release_task`, `devcouncil_renew_lease`

**Policy-gated writes:** `devcouncil_write_file`, `devcouncil_apply_patch`,
`devcouncil_policy_check_write` (preflight), `devcouncil_run_command`,
`devcouncil_record_command`

**Read-only inspection:** `devcouncil_read_file`, `devcouncil_get_diff`,
`devcouncil_get_gaps`, `devcouncil_get_next_actions`, `devcouncil_get_evidence`,
`devcouncil_get_task_provenance`

**Verification:** `devcouncil_verify_task` (requires an active lease)

**Live review:** `devcouncil_live_review`, `devcouncil_live_cards`,
`devcouncil_live_repair_prompt`

## Subagents

When delegated, use the bundled subagents:

- **devcouncil-implementer** — checkout → scoped edits → verify → release
- **devcouncil-verifier** — read-only verification and gap reporting
- **devcouncil-reviewer** — policy-aware diff review via live critique cards

## Rules of engagement

- **Scope:** edit only files declared in the task's planned scope. Out-of-scope or
  protected paths are rejected by the write gate.
- **Evidence:** run tests and call `devcouncil_verify_task` before claiming done.
- **Leases:** one agent owns a task at a time; checkout before writes, release when verified.
- **Repairs:** when gaps exist, read `devcouncil_get_next_actions` and act on each
  typed action — do not declare success while blocking gaps remain.

For the full autonomous loop, follow the **devcouncil-hero-loop** skill. For verifier
gates and the next-actions contract, follow **devcouncil-verification**.
