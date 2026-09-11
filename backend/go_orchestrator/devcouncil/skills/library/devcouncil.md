---
name: devcouncil
title: DevCouncil Integration for Claude Code
description: Operate inside a DevCouncil-managed repo — use MCP tools and slash commands for status, scope, policy-gated writes, and evidence-first verification instead of guessing project state.
triggers:
  keywords: [devcouncil, dev council, devcouncil mcp, mcp devcouncil, dev integrate]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Integration for Claude Code

This repository uses **DevCouncil** as a **component and module layer** for coding agents — leases, verification, and code intelligence — not as a required end-to-end application. **Manvi** wraps those components into a harness. Host apps such as GitPulse select Manvi and DevCouncil modules independently. Evidence — not model confidence — decides when work is done.

Check `gates.mode` before assuming verification is mandatory: `enforce` blocks,
`advisory` records non-safety findings without blocking, and `off` skips quality
verification. Hard-safety policy remains active in every mode.

Tasks, leases, and verification are an opt-in workflow outside `enforce`. In
`gates.mode=off`, ordinary edits and commands may proceed without creating,
checking out, or verifying a DevCouncil task. Use task-loop tools only when the
user explicitly asks for that workflow.

## Navigate before you edit

1. Open `.devcouncil/repo_map.json` for subsystem entry points, critical files, and
   cross-subsystem handoff paths. Regenerate with `dev map` after large refactors.
2. Read `AGENTS.md` / `CLAUDE.md` for workspace conventions.
3. Check status: `devcouncil_status` (MCP) or `/devcouncil:status` (slash command) or
   `dev status` (CLI).

## Dead code / liveness

Prefer `dev map dead --confidence extracted` plus file greps. Treat `inferred` as
unconfirmed. If `entry_roots` are empty or `liveness_unreachable_unreliable` is set,
**ignore** `unreachable_files` and mass inferred dead. Map `dead_symbol_candidates`
are extracted ∩ token-scan (methods excluded). Prefer `dev map query|trace|dead`
and `dev map graph-html` for symbol navigation / visualizer.

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

**Verification:** `devcouncil_verify_task` (a lease is required in `enforce` mode)

**Live review:** `devcouncil_live_review`, `devcouncil_live_cards`,
`devcouncil_live_repair_prompt`

## Subagents

When delegated, use the bundled subagents:

- **devcouncil-implementer** — checkout → scoped edits → verify → release
- **devcouncil-verifier** — read-only verification and gap reporting
- **devcouncil-reviewer** — policy-aware diff review via live critique cards

## Rules of engagement

- **Scope:** edit only files declared in the task's planned scope. When write-gates /
  contain mode are active, out-of-scope or protected paths are rejected by the write gate.
- **Evidence:** in `enforce`, run tests and call `devcouncil_verify_task`.
  In `advisory`, verification is optional and findings cannot block (except hard
  safety). In `off`, skip quality verification and report completion as unverified.
- **Interactive Shell:** under assist defaults (no `--write-gate`, `hook_gate.mode=off`),
  Cursor/Claude Shell does **not** need a lease — do not block on checkout for ad-hoc commands.
- **Leases:** one agent owns a task at a time; checkout before MCP gated writes / verify when
  write-gates / contain mode are active (or when running the hero loop), then release.
- **Repairs:** in `enforce`, read `devcouncil_get_next_actions` and close effective
  blocking gaps. Outside enforce, stored quality gaps are advisory history, not a
  reason to block ordinary work.

For the full autonomous loop, follow the **devcouncil-hero-loop** skill. For verifier
gates and the next-actions contract, follow **devcouncil-verification**.
