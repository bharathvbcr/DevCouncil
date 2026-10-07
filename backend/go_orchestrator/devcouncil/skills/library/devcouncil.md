---
name: devcouncil
title: DevCouncil Integration for Claude Code
description: Operate inside a DevCouncil-managed repo — use the DevCouncil MCP tools and the `dev` CLI for gate mode, scope, policy checks, and evidence-first verification instead of guessing project state.
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
3. Read the gate mode with `dev gate status` (add `--json` for scripting).

## Dead code / liveness

Use `dev map dead --json` and read each row's own `confidence`: a high-confidence row
has no inbound evidence after the call walk, a low one is unconfirmed. If `entry_roots`
are empty or `liveness_unreachable_unreliable` is set, **ignore** `unreachable_files`.
Map `dead_symbol_candidates` are extracted ∩ token-scan (methods excluded). For symbol
navigation use `dev map search`, `dev map explore` and `dev map trace`; `dev map html`
renders the graph for a browser. `dev map` passes its arguments to the `devmap` binary,
so `devmap --help` lists the rest.

## MCP tools vs CLI

| Need | Prefer |
|---|---|
| Task loop (checkout, verify, release) | MCP tools (`mcp__devcouncil__devcouncil_*`) |
| Gate mode | `dev gate status` |
| Scripting, CI, headless runs | `dev verify TASK-ID --json` |
| Read-only inspection (gaps, diff) | MCP read tools before re-running verify |

MCP tool names in Claude Code are prefixed: `mcp__devcouncil__devcouncil_<name>`.
Install the MCP server for a host with `dev integrate claude --apply`.

## MCP tools (by role)

This host serves exactly eight tools; `tools/list` is generated from
`devcouncil/registry.go`'s `toolSpecs()`, so it can report no others.

**Task loop:** `devcouncil_next_task`, `devcouncil_checkout_task`,
`devcouncil_renew_lease`, `devcouncil_release_task`

**Inspection:** `devcouncil_get_diff`, `devcouncil_get_gaps`

**Policy:** `devcouncil_policy_check_write` — a preflight that answers whether a
write would be allowed; it does not perform the write

Pass `task_id` (and `operation`: `create`, `modify` or `delete`) to have the path
judged against that task's planned scope. Without `task_id` the answer stops at
`task.absent` after the secret, restricted and outside-root rules.

**Verification:** `devcouncil_verify_task` (a lease is required in `enforce` mode)

Nothing else is served. Read files and run commands with your host's own tools,
read persisted state from `.devcouncil/` or `dev verify TASK-ID --json`, and take
`next_actions` from the `devcouncil_verify_task` result.

## Subagents

When delegated, use the bundled subagents:

- **devcouncil-implementer** — checkout → scoped edits → verify → release
- **devcouncil-verifier** — read-only verification and gap reporting
- **devcouncil-reviewer** — policy-aware diff review, with structure from DevMap

## Rules of engagement

- **Scope:** edit only files declared in the task's planned scope. No host hook
  enforces this — write-gates and contain mode are retired — so honour it yourself,
  and use `devcouncil_policy_check_write` when you want a path checked. A task that
  needs a wider scope is a question for its owner: no tool widens planned files.
- **Evidence:** in `enforce`, run tests and call `devcouncil_verify_task`.
  In `advisory`, verification is optional and findings cannot block (except hard
  safety). In `off`, skip quality verification and report completion as unverified.
- **Interactive Shell:** host lifecycle hooks are retired and nothing gates a write,
  Cursor/Claude Shell does **not** need a lease — do not block on checkout for ad-hoc commands.
- **Leases:** one agent owns a task at a time; checkout before verification (or when
  running the hero loop), then release.
- **Repairs:** in `enforce`, read the typed `next_actions` out of the
  `devcouncil_verify_task` result — or `devcouncil_get_gaps` for the persisted
  list without re-verifying — and close effective blocking gaps. Outside enforce, stored quality gaps are advisory history, not a
  reason to block ordinary work.

For the full autonomous loop, follow the **devcouncil-hero-loop** skill. For verifier
gates and the next-actions contract, follow **devcouncil-verification**.
