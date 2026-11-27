---
name: devcouncil
title: DevCouncil Integration for Claude Code
description: Operate inside a DevCouncil-managed repo — use the DevCouncil MCP tools and the `dev` CLI for gate mode, scope, policy checks, and evidence-first verification instead of guessing project state.
triggers:
  keywords: [devcouncil, dev council, devcouncil mcp, mcp devcouncil, dev integrate]
  markers: [.devcouncil/config.yaml]
---

# DevCouncil Integration for Claude Code

This repository uses **DevCouncil** as a **component and module layer** for coding agents — leases, verification, and code intelligence — not as a required end-to-end application. **Manvi** wraps those components into a harness. Host apps such as **Waylode** (formerly GitPulse) select Manvi and DevCouncil modules independently. Evidence — not model confidence — decides when work is done.

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
3. Read the gate mode with `dev gate status --json`; `dev doctor --json` says which task store `next_task` reads.
4. Navigate with `devmap search|explore|affected … --json --root <abs repo path>` (the shell form of the `devmap_*` MCP tools).
5. Check concurrent worktrees and collisions with **Waylode Insights** (`waylode_insights`, `waylode_collision_risk`; legacy `gitpulse_*` aliases remain accepted) before editing files another lane may hold.

## Dead code / liveness

Use `dev map dead --json` and read each row's own `confidence`: a 0.9 row has no
inbound evidence after the call walk *and* its language has earned the confident
tier against a labelled fixture and corpus record; a 0.4 row is unconfirmed —
including `confident_tier_unproven` (a language that has not earned the tier),
`external_supertype` / `object_literal_member` (something outside the graph may
call it by name) and `exported_unreferenced` (an export the manifest or the
language proves module-private). A 0.4 row's
`evidence` names the unbound sites and the first `path:line`; check it before
acting. `excluded` counts, per language, the uncalled symbols that were never
examined — an empty list over a mostly-exported corpus is not "no dead code". If `entry_roots`
are empty or `liveness_unreachable_unreliable` is set, **ignore** `unreachable_files`.
Map `dead_symbol_candidates` are extracted ∩ token-scan (methods excluded). For symbol
navigation use `dev map search`, `dev map explore` and `dev map trace`; `dev map html`
renders the graph for a browser. `dev map` passes its arguments to the `devmap` binary,
so `devmap --help` lists the rest.

## MCP tools vs CLI

| Need | Shell form (lead with this) | MCP alternative |
|---|---|---|
| Any `devcouncil_*` tool | `dev tool call <name> --args '<json>'` | `mcp__devcouncil__devcouncil_<name>` |
| Gate mode | `dev gate status --json` | none |
| Verify a task | `dev verify TASK-ID --json` | `devcouncil_verify_task` |
| Persisted gaps, diff | `dev tool call devcouncil_get_gaps --args '{"task_id":"T-1"}'` | `devcouncil_get_gaps`, `devcouncil_get_diff` |
| Health | `dev doctor --json` | none |
| Code navigation | `devmap search … --json --root <abs>` | `devmap_*` tools |

MCP tool names in Claude Code are prefixed: `mcp__devcouncil__devcouncil_<name>`.
Install the MCP server for a host with `dev integrate claude --apply`.

**Claude Code Code Mode.** When the host gives you a REPL tool that runs JavaScript
([Code Mode](https://code.claude.com/docs/en/code-mode)), fan the reads out in one
script and return only what blocks. `2>/dev/null` keeps the fallback notice that
`dev tool call` prints on stderr out of the JSON:

```js
const root = '/abs/repo', task = 'T-1', files = ['path/changed.go'];
const j = async (p) => { const r = await p.nothrow(); // exit 1 refused, 2 malformed, 3 could not run
  try { return { exit: r.exitCode, ...JSON.parse(r.stdout) }; } catch { return { exit: r.exitCode, raw: r.stdout.slice(0, 300) }; } };
const [gate, gaps, tests] = await Promise.all([
  j($`dev gate status --json --project-root ${root} 2>/dev/null`),
  j($`dev tool call devcouncil_get_gaps --args ${JSON.stringify({ task_id: task })} --project-root ${root} 2>/dev/null`),
  j($`devmap affected ${files} --json --root ${root} 2>/dev/null`),
]);
({ mode: gate.verification_mode, gaps_exit: gaps.exit, blocking: gaps.gaps?.filter((g) => g.blocking),
   tests: tests.tests?.items?.map((t) => t.path) })
```

A non-zero `gaps_exit` or a `raw` field means the call did not answer: never read it as "no gaps".

**When MCP is unreliable.** `dev tool call <name> --args '<json>'` runs the same
tool, from the same registry, with the same arguments, and prints the text an MCP
client would receive; `dev tool list` names the tools. Leases live in the task
store, so a lease taken over MCP is renewed, verified and released from the shell
and the other way round; keep one `client_id`. Exit status: 0 answered (for verify,
read `passed`), 1 refused, 2 malformed call, 3 could not run here. Say in your report when you fell back.
`dev doctor --mcp` starts each MCP server, times its handshake and `tools/list`,
and says whether the served tools match this binary.

## MCP tools (by role)

This host serves exactly eight tools; `tools/list` is generated from
`devcouncil/registry.go`'s `toolSpecs()`, so it can report no others.
Every tool accepts an optional `repo_path` (alias `root`) argument naming the
absolute repository path so a single global `devcouncil mcp` server can serve
multiple repositories.

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
