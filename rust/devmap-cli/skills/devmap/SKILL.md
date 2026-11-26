---
name: devmap
description: Use DevMap first for code navigation, callers, blast radius, traces,
  dead code, and code structure. Report incomplete evidence and use a justified fallback
  when necessary.
---

# Code navigation with DevMap

Resolve this checkout with `devmap paths --json`, then check `devmap status --json`. Read the returned `repo_map` path; do not assume `.devcouncil` rather than `.devmap`. A missing, stale, or partially parsed index is not complete evidence. Rebuild with `devmap build --manifest` when needed and authorized, then recheck status.

Lead with the CLI: `devmap <cmd> … --json --root <absolute repo path>` works in any host and reads this checkout's own store. The `devmap_*` MCP tools named below are the alternative when the MCP server is connected and targets this checkout; on those calls (and `waylode_codeintel_*` (or the old `gitpulse_codeintel_*`)) always pass `repo_path` (the absolute repository path) and check `repository.root` in the envelope before trusting the answer — Cursor shares one `devmap mcp` process across tabs. A host need not expose every CLI capability or MCP tool; check `devmap --help` and the tool schemas.

| Question | CLI | DevMap MCP, when available |
|---|---|---|
| Definition, callers, and callees | `devmap explore <name> --json` | `devmap_explore` |
| Find a name | `devmap search <name> --json` | `devmap_search` |
| Where a behaviour lives, with source and tests | `devmap ask --evidence "<question>" --json` | `devmap_ask_evidence` |
| Dependencies | `devmap deps <target> --json` | `devmap_dependencies` |
| Blast radius | `devmap impact <target> --json` | `devmap_impact` |
| Call chain | `devmap trace <from> <to> --json` | `devmap_trace` |
| Candidate tests | `devmap affected <target> --json` | `devmap_affected_tests` |
| Dead-code candidates | `devmap dead --json` | `devmap_dead_symbols` |
| Docs that name code which moved or is gone | `devmap doc-drift --json` | `devmap_doc_drift` |
| Proposed edit | `devmap preview --help` for installed syntax | `devmap_preview` |

Read `available` or `resolution`, `reason`, `shown`, `total`, `truncated`, and `walk_incomplete` wherever present. Dead rows carry their own confidence. 0.9 means no inbound evidence *and* the symbol's language has earned the confident tier against a labelled fixture and a labelled corpus record; any other language's would-be 0.9 row comes back at 0.4 with `confident_tier_unproven`. Rows at 0.4 are unconfirmed — `only_ambiguous_callers`, unresolved namesake, coverage-capped, `confident_tier_unproven`, `external_supertype` (a method on a class inheriting from a library type whose contract DevMap does not know, which may call it by name), `object_literal_member` (reached through the object, by key), or `exported_unreferenced` (an export the manifest or the language proves module-private) — and carry `evidence` with the unbound-site count and the first `first_path:first_line` to check. `excluded` counts, per language, the uncalled symbols never examined (`not_examined` for visibility, `exempt` by reason): an empty or short list over a mostly-exported corpus is not "no dead code". `NoNamesake` is an explained ledger gap, not a dead finding. A partial walk can omit callers even when `truncated` is false. An evidence pack's `related_tests` and `role` come from call edges and test-runner annotations, not from running anything. Candidate tests are not measured test coverage; direct callers may be affected, not necessarily broken. Check source and actual tests before claiming a cause or safe deletion.

## Code Mode (hosts with a JavaScript REPL tool)

Where the agent writes JavaScript in a REPL (Claude Code's Code Mode), call the CLI as `$` shell commands, fan independent queries out with `Promise.all`, and return only what you need to read. `$` passes an array value as separate arguments. Both answers carry `truncated` and `walk_incomplete`; return them with the rows. `status --json` is large (about 27 KB), so pick the fields you need in the script (`query_ready`, `is_fresh`, `rebuild_required`, `schema_outdated`, `degraded_reason`) instead of returning it whole.

```js
const R = '/abs/repo';
const q = (args) => $`devmap ${args} --json --root ${R}`.json();
const [imp, aff] = await Promise.all([q(['impact', 'Sym']), q(['affected', 'Sym'])]);
({
  impact: imp.items?.slice(0, 20),
  tests: aff.tests?.items,
  truncated: [imp.truncated, aff.tests?.truncated],
  walk_incomplete: [imp.walk_incomplete, aff.tests?.walk_incomplete],
})
```

Without Code Mode, run the same commands one at a time.

Prefer DevMap for the questions it can answer. If a required capability is absent, the index is unusable, or the answer is materially incomplete, state the specific limitation and use direct source inspection. Do not run a second index routinely. Follow explicit user and repository requirements. Check `devmap --help` and connected tool schemas before declaring a capability absent; CLI and MCP capabilities differ.

Report gaps in the task's audit or response. A skill does not authorize creating telemetry files, starting watchers, rebuilding repeatedly, or changing another host's configuration.
