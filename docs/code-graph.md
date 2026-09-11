# Repository Map & Code Graph

DevCouncil builds a deterministic repository map and a symbol-level knowledge graph without calling an LLM. Agents use these artifacts to navigate, find callers, and spot unwired or dead code before inventing new modules.

<p align="center">
  <img src="graph_preview.png" alt="Code Graph Preview" width="600">
</p>


## CLI umbrella

All map and graph operations live under **`dev map`**. `dev graph …` is a compatibility alias for the same command group.

| Prefer | What it runs |
| :--- | :--- |
| `dev map` (no args) | `devmap build --manifest` |
| `dev map query` / `trace` / `dead` / … | forwarded when the `devmap` subcommand exists (`search`, not `query`) |
| `dev map html` | `devmap html` — **symbol graph**, not the subsystem map |
| `dev map map-html` | `devmap map-html` — subsystem map from `repo_map.json` |
| `dev map ingest` / `init` / `sync` / `watch` / `demo` / `view` / `graph-html` | **Retired** or unknown. Use `build`, `serve`, `html`, `map-html`. |
| `dev map pdg FILE` | `devmap pdg FILE` (Python-only, intra-procedural; optional `--taint`) |

**HTML split:** `devmap map-html` → `.devcouncil/map.html`. `devmap html` / Go `dev map html` → symbol graph HTML. There is no `graph-html` or `--symbols` flag.

Rust-generated agent guides also include the portable [repository hygiene policy](repository-hygiene.md). The policy is reusable by embedding hosts; scheduling and deletion remain host responsibilities.

## Artifacts

| Path | Role |
| :--- | :--- |
| `.devcouncil/repo_map.json` | File inventory, subsystems, entry roots, unwired/unreachable/dead-symbol candidate lists, reverse-import dependents |
| `.devcouncil/graph/code_graph.json` | Compact export of symbol nodes + edges (imports, named imports, calls, inherits, contains) and tiered `dead_code`, written by the kernel from the same generation as the map. **The kernel store is canonical**; prefer `devmap search` / `explore` / `trace` / `dead` when the JSON is missing. |
| `.devcouncil/codeintel/devmap.sqlite` | **Canonical.** The Rust kernel's WAL-mode store. Written by `devmap build` (Go `dev map` execs that binary). There is no MCP `devcouncil_graph_ingest` on the Go host. |
| `.devcouncil/codeintel/index.sqlite` | **Retired.** Was the Python query cache and DAP tracer store. The debug broker is gone; nothing in the native host creates this file. Safe to delete. |
| `.devcouncil/graph/graph.html` | Self-contained symbol-graph visualizer (`devmap html` / `dev map html`) |
| `.devcouncil/map.html` | Subsystem map (`devmap map-html`). Nodes coloured by dominant language; header carries a repo-wide language bar. |
| `AGENTS.md` / `CLAUDE.md` | Marker-guarded workspace guides (`devmap build --manifest --guides`) |

## Build / refresh

```bash
dev map                     # Execs `devmap build --manifest`
dev map --full              # Same, plus `--full` (cold rebuild)
dev map status              # `devmap status`
dev map doctor              # `devmap doctor` (no `--fix` flag)
dev map html                # Symbol graph HTML (`devmap html`)
dev map map-html            # Subsystem map HTML (`devmap map-html`)
dev map serve               # Watcher + IPC (`devmap serve`); not `dev map watch`
dev map mcp                 # DevMap MCP stdio (`devmap mcp`); not `serve --mcp`
dev map history             # Recent builds
dev map repair              # Drop unindexable pending-queue rows
```

Python-era flags that are **not** on this binary: `--wiki` / `--no-wiki`, `--scan-deps`, `--pdg` as a build flag, `--if-stale`, `init`, `ingest`, `sync`, `watch`, `abort`, `runs`, `demo`, `view`. PDG is `devmap pdg`. SCA is not a `devmap` build step (GitPulse Health is a related job).

`--no-liveness` and `--lsp-refs` are gone: the kernel always computes liveness, and the LSP adjunct was cut with the Python engine. A flag that is accepted and ignored is worse than one that is rejected, so both are rejected.

### One writer

The kernel is the only writer of `repo_map.json` and `code_graph.json`. The current command is `devmap build --manifest` (Go `dev map` / `dev graph` execs `devmap`). Python `dev map init|ingest|sync`, `dev plan`, `dev init`, verify/checkout map refresh, and MCP `devcouncil_graph_ingest` were deleted with Phase 7. Guides (`AGENTS.md` / `CLAUDE.md`) are written by `devmap build --manifest --guides` or `devmap integrate`.

### When a map looks wrong — for a person or an agent

Use **`devmap status --json`** and **`devmap doctor --json`**. `devmap history` lists recent builds. There is no `dev map abort`, `dev map runs`, or `devmap doctor --fix`. A stuck writer is an OS-level process against `devmap.sqlite`; `devmap serve` is the long-lived watcher.

Doctor/status live on the **Rust CLI**. There are no Go-host MCP tools named `devcouncil_graph_doctor`, `devcouncil_graph_runs`, `devcouncil_graph_ingest`, or `devcouncil_tail_trace`. Use `devmap_*` MCP or the CLI. Host hooks that ran `dev map --if-stale --no-wiki` are retired.

Failure codes a build can raise: `binary_missing`, `schema_newer_than_kernel`, `store_locked` (another writer holds the store), `store_corrupt`, `store_unwritable`, `kernel_flag_unsupported`, `kernel_timeout`, `kernel_failed`.

HTML visualizers: `devmap map-html` writes `.devcouncil/map.html` (subsystem map, no store required). `devmap html` / Go `dev map html` writes the symbol graph. There is no `dev map demo` / `dev map view` command.

## Sample graph HTML

```bash
devmap html --root /path/to/repo
devmap map-html --root /path/to/repo
# Go aliases: `dev map html`, `dev map map-html`
```

## PDG / CFG / taint (opt-in, Python source only)

`devmap pdg <FILE>` reads the file from disk (not the index). Optional `--taint` filters to heuristic sinks. There is no `dev map --pdg` build flag, no `pdg build` / `explain` / `pdg-query` subcommands, and no write into `index.sqlite`. Not on DevMap MCP (`MISSING_CAPABILITIES`).

**Limitations (MVP)**

1. Intra-procedural only — no cross-function or inter-file taint.
2. Python-first — TS/JS deferred.
3. No field-sensitive or alias analysis.
4. CDG branch sense is coarse (`if`/`while` only; `match` arms treated uniformly).
5. Taint uses pattern tables — expect false positives/negatives.
6. Opt-in per file — default `devmap build` does not run PDG.

## Query the graph

```bash
devmap search refresh_map_artifacts
devmap explore refresh_map_artifacts --json
devmap trace path/a.py path/b.py
devmap dead --json
devmap impact src/foo.py --json
devmap html
devmap map-html
devmap export
devmap search "auth flow" --semantic
devmap cypher 'MATCH (a)-[r:CALLS]->(b) RETURN a.id, b.id LIMIT 20'
devmap affected request_handler
```

Unknown on current `devmap`: `query`, `check`, `process`, `view`, `demo`, `graph-html`. Use `search`/`explore` instead of `query`.

`explore` and `affected` moved into the kernel on 2026-09-05, replacing a Python
engine that loaded the whole graph into process memory. Both take a symbol or a
`path::symbol` — `affected` resolves its targets through the same traversal
matcher `impact` uses, so a bare file path works only when the graph has an edge
touching that file. Three contract changes came with the move, designed into the engine:

- `explore` ranks its matches before the cut and reports the index-wide match
  count, not the size of the page it returned; each caller/callee list keeps its
  own exact total, so `0 shown of 42` never reads as "no callers".
- A source snippet that could not be read carries `source_unavailable_reason`
  instead of coming back as an empty body.
- `affected` ranks nearest-first, carries each test's distance, and names targets
  that matched nothing rather than folding them into "no affected tests".

## How a build works

`devmap build` extracts every source file with tree-sitter (30 linked grammars; grammarless declaration languages such as `.proto` and `.ps1` get pattern-recovered symbols stamped as fallback; prose and data formats contribute a `File` node and are not counted as parse failures), resolves imports and calls across the whole tree, runs liveness and community detection, and persists one generation in a single transaction. An unchanged tree (same content hashes, same kernel) returns without writing; a changed one carries unaffected files forward and rewrites the affected closure. The store keeps two generations, prunes the extraction cache to what they reference, and reclaims free pages with an incremental vacuum followed by a WAL checkpoint — on the unchanged path too, so a quiet repository does not sit at 40% free pages.

Concurrent writers (`dev map` in two shells, a daemon drain and a build) serialise on an advisory file lock next to the store with a bounded wait; the loser fails with the holder's pid rather than a bare `database is locked`, and the lock is released by the operating system when the holder dies.

The daemon (`devmap serve`, started on demand by the Python client for queries) watches the tree, coalesces events, and drains pending paths into the same store. Paths it cannot process — outside the root, no longer existing, oversized, not a source file — are dropped, not retried forever; `dev map status` names any that remain quarantined and `dev map repair --pending` drops them. The daemon retires after 30 idle minutes, when its executable changes, on SIGTERM, and when its root or store disappears.

### File inventory

The freshness fingerprint and the goal ranking are computed over the git inventory (`RepoMapper.get_git_files`):

- `indexing.include_untracked` (default **on**) — include untracked-but-not-ignored files. **Keep this on**: a file an agent just wrote and has not staged is otherwise absent from the fingerprint, so a build after that write reads fresh.
- `indexing.max_indexed_files` (default 50000) — hard ceiling on the inventory. Untracked paths are dropped first and the overflow is logged, never silently truncated. Generated trees (`node_modules`, `target`, `coverage`, `vendor`, `Pods`, `.next`, `.devcouncil`, binaries, archives, …) are excluded at any depth regardless.

## Code Intelligence Architecture (`devmap`)

The `dev map` engine is **`devmap`**, a compiled multi-crate architecture:

| Crate | Responsibility |
| :--- | :--- |
| `devmap-extract` | tree-sitter parsing, wiring annotations, framework matchers, explicit unavailable outcomes |
| `devmap-resolve` | import, call, and receiver-type resolution with typed confidence |
| `devmap-analyze` | liveness / dead-code tiers and weighted communities |
| `devmap-store` | rusqlite schema, pending queue, history, differential writes |
| `devmap-query` | token-budgeted search / deps / manifest |
| `devmap-serve` | file watcher and durable pending drain |
| `devmap-cli` | the `devmap` command-line binary |

**The native engine is the production map engine; there is no Python fallback for building.**
`dev map` / `dev graph` / `dev ast` exec the `devmap` binary. Locate `devmap` via `DEVMAP_BIN` / `PATH` / `~/.local/bin` (the npm shim and Go host both resolve this).

Build and run `devmap` directly:

```bash
bash scripts/install-components.sh devmap
devmap --version
devmap build --manifest
devmap search helper --budget 500
```

## Dead-code confidence tiers

| Tier | Meaning |
| :--- | :--- |
| `extracted` | No inbound call/import edges in the resolved graph **and** token-scan agrees |
| `inferred` | Only callers are themselves dead (transitive island), or methods with no inbound calls |
| `ambiguous` | Graph-dead but token-scan or name-only refs suggest a possible false positive |

Framework-invoked symbols are kept live by the kernel, which resolves them during the
build rather than in a Python enrichment pass afterwards. Two mechanisms do it. Edges:
an unambiguous `routes_to` (kernel `EdgeKind::HandlesRoute`) or `subscribes`
(`SubscribesTo`) makes its handler target live even though nothing calls it from source.
Exemptions: `WiringKind` marks a symbol the runtime invokes with no observable call site
at all — `FrameworkDecorator`, `RuntimeEntryPoint` (`func init`, `#[test]`,
`componentDidMount`, `pytest_*`), `ScriptEntry`, `Launcher`, `ReExportPackage`,
`ConfigEntryPoint` (a `[project.scripts] cli = "pkg.mod:func"` declaration, resolved to
`pkg/mod.py::func` — the launcher that calls it is generated at install time and is not
in the corpus), `StructuralExempt` (a Rust trait-impl method cannot carry `pub`, so
`is_exported` says nothing about it) — and an exempt symbol reports the reason it was
exempted, per symbol or per file. `ScriptEntry` and `ConfigEntryPoint` come from the same
declaration and answer different questions: the first is a claim about the manifest
*file*, the second about the symbol it names. Ambiguous name matches stay unresolved and never suppress a dead-code
candidate.

The Python framework manifest that used to do this — `codeintel/resolution/frameworks/`,
with its routing/DI/event matchers and one fixture per advertised family — was retired
with `build_code_graph` on 2026-09-02. Two of its outputs have no kernel equivalent
today and are therefore absent, not merely quiet: `provides` and `listens` edges (DI
providers and observer registration), and the `registers` edge that let liveness follow
a registration owner through a registration node, so that a route registered inside a
dead setup function stayed dead. Both had already stopped being produced when the kernel
became the sole writer; the deletion only made that visible. Restore them in the kernel,
not in Python, if they are wanted back.

Prefer `dev map dead --confidence extracted` plus file greps before deleting anything.
Treat `inferred` as **unconfirmed**. If `entry_roots` are empty or
`liveness_unreachable_unreliable` is set, **ignore** `unreachable_files` and mass inferred dead.

`dev map` stores a **capped** (5000) `dead_symbol_candidates` list for agents:
**extracted ∩ token-scan** (methods excluded). `dev map dead` reports the **uncapped**
graph tiers with reasons. Prefer reviewing `ambiguous` before deleting anything. A failed
build leaves the prior artifacts byte-identical rather than stamping a lean map over them —
the Python `assemble_graph` fallback that used to omit dead-symbol candidates on failure went
with the retired builder, and the kernel fails closed in its place.

Map liveness lists (`unwired_candidates`, `unreachable_files`, `dead_symbol_candidates`) are
capped at **5000** each; `dependents[path]` is capped at **256** per file. When a list hits
its cap, metadata records `*_truncated` plus totals — use `dev map dead` for uncapped tiers.

## Liveness lists on the map

- **entry_roots** — configured + convention entry points (CLI mains, `__main__`, etc.)
- **unwired_candidates** — code files with no non-test importers (capped at 5000)
- **unreachable_files** — not reachable via imports from production entry roots (capped at 5000).
  Empty / unreliable when production entry roots are missing (`liveness_unreachable_unreliable`)
- **dead_symbol_candidates** — extracted ∩ token-scan (methods excluded; capped at 5000)
- **liveness_unreachable_unreliable** — true when unreachable BFS was skipped (empty prod roots)
- **dependents** — reverse-import index per file (capped at 256 importers per path)

### Map ↔ verify asymmetry (same-task island)

Map unwired lists treat **any** non-test importer as clearing unwired. Verify’s unwired gate for *new* files is stricter: a file imported only by other files added in the same task still fails (same-task island rule) until a **pre-existing** non-test caller imports it. Map lists are navigation hints; verify is the gate.

Verification can also enforce wiring / stale-map / dead-symbol / liveness-ratchet checks when those gates are enabled. **Write policy** soft-blocks edits outside `planned_files` unless the target is in the same subsystem or a map `neighbors` area — escape via `dev scope update` / `devcouncil_update_task_scope`.

## Agent workflow

1. Open `.devcouncil/repo_map.json` before guessing file locations.
2. Use `subsystems` → `entry_points` / `critical_files` / `role_files` / `neighbors`.
3. Prefer `devmap dead --json` + greps; read each row's `confidence`. Treat inferred as unconfirmed.
   If entry roots are empty / unreliable, ignore unreachable and mass inferred dead.
   Check `unwired_candidates` / `dead_symbol_candidates` before adding modules.
4. Use `devmap search` / `explore` / `trace` / `dead` for symbol-level navigation.
5. Run `devmap build --manifest` (or `dev map`) after large refactors. There is no `--watch` flag; `devmap serve` is the long-lived watcher.

## API route mapping

Native HTTP surface tools over `ROUTE` nodes and `routes_to` /
`registers` edges (no external graph service):

```bash
devmap routes --json
devmap shape-check --json
devmap api-impact /api/items --json
```

- **`routes`** — handlers, registration owners, and client fetch sites
  (`fetch`, `axios`, `requests`, `httpx`) matched by normalized path
  (`:param`, `{id}`, `[id]` → `*`).
- **`shape-check`** — handler return dict keys (Python AST / TS regex) vs keys
  accessed on the response variable in a short post-fetch window.
- **`api-impact`** — consumers, middleware registrations, shape mismatches, and
  a risk tier (`high` / `medium` / `low` / `none`).

Those three are **CLI-only** on current `devmap` (`routes`, `shape-check`, `api-impact`). They are not Go-host MCP tools. DevMap MCP (`devmap mcp`) does not advertise them (`MISSING_CAPABILITIES` in `rust/devmap-cli/src/session.rs`). There is no `devcouncil_graph_ingest` / `devcouncil_graph_cypher` / `devcouncil_explain` on the Go host.

## Corpus side index

Retired. See [corpus.md](corpus.md). There is no `dev corpus` command.
