# Repository Map & Code Graph

DevCouncil builds a deterministic repository map and a symbol-level knowledge graph without calling an LLM. Agents use these artifacts to navigate, find callers, and spot unwired or dead code before inventing new modules.

<p align="center">
  <img src="graph_preview.png" alt="Code Graph Preview" width="600">
</p>


## CLI umbrella

All map and graph operations live under **`dev map`**. `dev graph …` is a compatibility alias for the same command group.

| Prefer | Alias (same behavior) |
| :--- | :--- |
| `dev map` | — (build repo map + code graph) |
| `dev map query` / `trace` / `dead` / … | `dev graph query` / `trace` / `dead` / … |
| `dev map ingest` / `init` / `sync` / `watch` / `doctor` | `dev graph ingest` / … |
| `dev map demo` / `view` / `export` / `search` / `cypher` | `dev graph demo` / … |
| `dev map graph-html` or `dev map html --symbols` | `dev graph html` |
| `dev map html` | subsystem map only (no graph alias) |
| `dev map pdg build` / `explain` / `pdg-query` | `dev graph pdg build` / … |

**HTML split:** `dev map html` writes the subsystem map (`.devcouncil/map.html`). Symbol-level graph HTML is `dev map graph-html`, `dev map html --symbols`, or alias `dev graph html` (`.devcouncil/graph/graph.html`).

## Artifacts

| Path | Role |
| :--- | :--- |
| `.devcouncil/repo_map.json` | File inventory, subsystems, entry roots, unwired/unreachable/dead-symbol candidate lists, reverse-import dependents |
| `.devcouncil/graph/code_graph.json` | Compact compatibility export of symbol nodes + edges (imports, named imports, calls, inherits, contains) and tiered `dead_code`. **SQLite is canonical**; this JSON is a size-sensitive export (`indexing.compact_graph_json`, default on; 128 MiB default limit). Oversized graphs fall back through slim → compact → stub tiers so a pointer JSON is still written; prefer SQLite-backed `dev map` commands when the stub tier is used. |
| `.devcouncil/codeintel/index.sqlite` | Canonical WAL-mode graph, source cache, FTS, generations, unresolved references, diagnostics, and fingerprinted runtime observations |
| `.devcouncil/graph/graph.html` | Self-contained interactive visualizer (`dev map graph-html` / `dev map html --symbols` / alias `dev graph html`; **not** written by default on bare `dev map`) |
| `.devcouncil/map.html` | Self-contained subsystem map visualizer (`dev map html`; slim payload — no `files[]` / `dependents{}`) |
| `.devcouncil/graph/demo.html` | Sample self-contained interactive UI from `dev map demo` (no map required; primary demo artifact) |
| `.devcouncil/graph/demo.svg` | Optional static companion written by `dev map demo` (not the interactive UI) |
| `AGENTS.md` / `CLAUDE.md` | Marker-guarded workspace guides kept in sync with the map |

## Build / refresh

```bash
dev map                     # Full rebuild (liveness on by default)
dev map --goal "…"          # Optional goal text for candidate-file ranking (was positional)
dev map --if-stale          # No-op when fingerprints still match
dev map --no-liveness       # Skip entry/unwired/unreachable/dead lists
dev map --lsp-refs          # Confirm dead symbols via live LSP references
dev map --wiki / --no-wiki  # Refresh codebase-wiki skeletons after map (on by default)
dev map --scan-deps         # Opt-in SCA (pip-audit / npm audit / osv-scanner) → dependency_risks
dev map --watch             # Debounced incremental refresh on edits (same as `dev map watch`)
dev map html                # Write interactive .devcouncil/map.html (subsystems)
dev map html --open         # Write and open the subsystem map
dev map graph-html          # Write symbol-level .devcouncil/graph/graph.html
dev map html --symbols      # Same as graph-html
dev map init                # Build canonical SQLite + compatibility exports
dev map ingest              # Unified analyze: codeintel sync → graph export → repo map write
dev map ingest src/foo      # Path-scoped ingest (full reconcile when paths omitted)
dev map status              # Generation, pending paths, watcher/degraded state
dev map --full              # Force a full isolated rebuild (default is incremental when the change set is small)
dev map unlock              # Free a stuck writer lease (dead / stalled / timed_out)
dev map unlock --force      # Kill even if the holder still looks progressive
dev map sync                # Reconcile and commit now
dev map watch               # Native FSEvents/inotify/ReadDirectoryChangesW foreground watcher
dev map doctor              # SQLite, watcher, and offline grammar verification
```

### Writer-lease recovery runbook

When `dev map` / `--if-stale` / verify remaps keep failing with `graph_writer_busy`, or `dev map status` shows a stalled build:

1. **Inspect** — `dev map status` (holder pid, phase, `stalled` / `timed_out`, last progress).
2. **Unlock** — `dev map unlock` (dead holder or stalled/timed-out). Use `dev map unlock --force` only if the holder still looks progressive but you need to reclaim the lease.
3. **Rebuild** — `dev map` or `dev map ingest` once the lease is free.
4. **Do not raw-kill** under contain write-gate — `kill` / `pkill` stay denied without a task lease. Prefer `dev map unlock` (lease-lifecycle allowlisted even with `task=None`).

Freshness uses git HEAD, a tracked-file hash, and a content fingerprint so plain edits mark the map stale. Fingerprint / git errors fail closed (treat as stale). A **missing** `.devcouncil/repo_map.json` is also stale — hard rigor blocks checkout/verify until `dev map` or `dev map ingest` runs. Post-tool-use hooks and `dev map --watch` refresh incrementally; incremental extract still verifies parse-cache sha256 so a concurrent edit to an unlisted path cannot stamp a fresh fingerprint over stale symbols.

HTML visualizers: set `indexing.write_graph_html: true` in config if you want bare `dev map` to also write `graph.html`. Otherwise use `dev map graph-html` / `dev map view` (or alias `dev graph html`) for the file/symbol graph, and `dev map html` for the subsystem map.

## Sample graph demo (no map)

```bash
dev map demo --project-root /tmp/devcouncil-docs-smoke --json
# Open /tmp/devcouncil-docs-smoke/.devcouncil/graph/demo.html
# Alias: `dev graph demo` (same command group)
```

`dev map demo` writes a **self-contained interactive HTML** page with a synthetic import graph. Open `demo.html` for filters, path highlighting, and neighborhoods. A static `demo.svg` may also be written; it is not a substitute for the interactive page. Supported platforms for the CLI and npm wrapper: macOS, Linux, and Windows (Node.js 18+, Python 3.12+, Git).

## PDG / CFG / taint (opt-in)

Program-dependence analysis is **off by default** and does not run during normal `dev map` unless you pass `--pdg`. It is Python-only and intra-procedural in the MVP.

| Layer | Scope | Artifact |
| :--- | :--- | :--- |
| CFG | per function | basic blocks + branch/fallthrough edges |
| Reaching-def | intra-procedural | def line → use line per variable |
| CDG | intra-procedural | controller block → dependent block (+ `guard` on early return) |
| Taint | heuristic | source→sink findings by category |

**Persistence**

- Summary + capped findings (≤500): `graph.meta["pdg"]` in `code_graph.json`
- Full per-file payload: `analysis_shards[path]["pdg"]` in `codeintel/index.sqlite`

**CLI**

```bash
dev map --pdg                                    # map + PDG in one shot
dev map pdg build --path src/foo/bar.py          # on-demand for paths
dev map explain --category command-injection
dev map pdg-query --mode controls --target my_fn
dev map pdg-query --mode flows --target my_fn --variable x
```

**Limitations (MVP)**

1. Intra-procedural only — no cross-function or inter-file taint.
2. Python-first — TS/JS deferred.
3. No field-sensitive or alias analysis.
4. CDG branch sense is coarse (`if`/`while` only; `match` arms treated uniformly).
5. Taint uses pattern tables — expect false positives/negatives.
6. Opt-in — default `dev map` unchanged without `--pdg`.

## Query the graph

```bash
dev map query build_code_graph     # definition + callers/callees/importers
dev map trace path/a.py path/b.py
dev map dead                       # full dead-code report (uncapped)
dev map dead --min-confidence inferred
dev map check                      # god nodes + circular imports
dev map process                    # BFS call-flows from entry roots
dev map impact src/foo.py          # blast radius
dev map impact --diff              # blast radius for working-tree changes
```

`dev map check` / `process` / `impact` (and PageRank inside god-node ranking) follow **extracted** and **inferred** import/call edges only. Ambiguous call fan-out stays in the store for explanation UIs but does not invent hubs or inflate blast radius.

```bash
dev map graph-html                 # write symbol graph.html
dev map html --symbols             # same as graph-html
dev map view                       # serve/open the HTML
dev map export -o out.graphml      # GraphML (or --format okf)
dev map search request_handler     # FTS5 symbol/path search
dev map search "auth flow" --semantic  # Opt-in local embeddings (indexing.embeddings.enabled)
dev map cypher 'MATCH (a)-[r:CALLS]->(b) RETURN a.id, b.id LIMIT 20'
dev map explore request_handler    # source + semantic paths + blast radius
dev map affected src/foo.py        # tests in the inbound impact closure
```

## Transactional code intelligence

SQLite is canonical; graph v2 JSON remains a deterministic compatibility export. A refresh writes a complete generation in one transaction and advances the current-generation pointer only after every file, node, edge, liveness record, and FTS row is committed. Readers therefore see the complete previous or complete next graph. The store retains two committed generations for rollback/debugging and caches compressed source and extraction facts by content, grammar, analyzer, and configuration hashes.

MCP starts one project watcher for its server lifespan. Queries wait up to two seconds for a pending batch without blocking the async server; if syncing cannot finish, responses retain the last committed generation and identify pending/degraded state. Full builds run in a supervised child that **acquires the per-project writer lease itself**; the parent releases any held lease while supervising so an orphaned worker still serializes against watchers/MCP writers. Lease acquisition uses bounded exponential backoff (`code_intelligence.writer_lease_timeout_seconds`, default 30s for builds / re-acquire; `writer_lease_sync_timeout_seconds`, default 5s for watch `sync_now`) so multi-watcher contention does not stamp lean/degraded maps over a healthy SQLite generation. After the child commits, the parent reloads the graph under the re-acquired lease. `dev map status` / `dev map doctor` expose phase, progress, worker PID, consumed worker CPU, and compatibility-export health (missing/drift/degraded/corrupt). `dev map doctor` reports `graph_ok` (canonical SQLite) separately from `json_export_ok` — a size-capped JSON export no longer marks an otherwise healthy graph as failed. External edits to `code_graph.json` do **not** clobber SQLite — the store wins unless the store is empty. `dev map watch` and `dev map --watch` both refresh graph **and** rebuild `repo_map.json` subsystems/dependents.

### Stall detection is CPU-aware

Long phases (liveness tokenize, semantic enrichment, SQLite persist) can run for many minutes without advancing a phase counter. The worker therefore emits a **timer-driven heartbeat carrying its consumed CPU time** every `indexing.build_heartbeat_interval_seconds` (default 5s), independent of phase counters, and every long phase also reports incremental progress (`liveness:tokens`, `semantic`, `persist:nodes`, `persist:edges`, `persist:files`, `export:json`).

The supervisor declares a stall only when phase progress **and** worker CPU are both flat past the budget, so a worker at 90%+ CPU is never killed as "hung"; a genuinely wedged (zero-CPU) worker still is. The applied budget is `max(indexing.build_stall_timeout_seconds, indexing.semantic_enrich_timeout_seconds + 30s)`, so a short stall timeout cannot expire inside the semantic budget. `indexing.build_total_timeout_seconds` (default 15 min) remains a hard ceiling. A timed-out worker's writer-lock metadata is cleared automatically, and a dead holder is reclaimed on the next acquire — recovery no longer requires a manual `dev map unlock`.

If a build does time out with a healthy committed generation, the map is refreshed **from that generation** rather than crashing the CLI: `dev map` prints the reason, exits non-zero, and keeps the prior graph's fingerprints so `--if-stale` correctly still reports stale.

### Incremental by default

`dev map` with no path arguments probes the change set against the committed generation using each file's recorded size/mtime (no hashing, no re-reads):

- **No changes** → the generation is reused and only the map artifacts are rewritten.
- **Small change set** (≤500 files and ≤20% of the tree) → incremental sync.
- **Larger, corrupt store, or `--full`** → supervised full rebuild.

Incremental sync itself is deliberately conservative. Body-only edits with an unchanged declaration/import resolution surface replace the affected closure in-process. Creates, deletes, renames, or changes to symbols, bases, decorators, exports, imports, re-exports, or aliases trigger a full resolve from warm extraction caches. Persisted analysis shards are pruned to the current non-vendored code-file set before either path. Configure the boundaries with `indexing.build_isolation: hybrid`, `indexing.build_stall_timeout_seconds`, `indexing.build_total_timeout_seconds`, and `indexing.graph_json_max_bytes`.

### Index size and file inventory

- `indexing.store_file_contents` (default **off**) — persist compressed file bytes in `file_contents`. Path, content hash, size, and mtime are always retained; with blobs off, `content_for_path` reads the working tree. Turning this on is what grows `index.sqlite` into the gigabytes on a large repo.
- `indexing.store_write_batch_size` (default 2000) — rows per batched persist write. Each batch also emits a progress heartbeat. The WAL is truncated after every committed generation so it cannot grow unbounded across runs.
- `indexing.include_untracked` (default **on**) — index untracked-but-not-ignored files. **Keep this on.** Turning it off does more than hide new files: a file an agent just wrote and has not staged leaves the graph entirely, so the tracked symbols it calls lose those call edges and surface as dead/unwired candidates. Tracked-only indexing produces *false dead-code signals*, not just a smaller index. The generated-tree filter and `max_indexed_files` are the real bound on inventory size.
- `indexing.max_indexed_files` (default 50000) — hard ceiling on the inventory. Untracked paths are dropped first and the overflow is logged, never silently truncated. Generated trees (`node_modules`, `target`, `coverage`, `vendor`, `Pods`, `.next`, binaries, archives, …) are excluded at any depth regardless.

A repo-scale change set is handed to the build worker through a file, not one `--changed-path` argv entry per path (which overflowed `ARG_MAX`), and the incremental membership copy stages large exclusion sets in a temp table instead of one bind parameter per path.

The 35-language grammar matrix is delivered through platform-specific
`devcouncil-codeintel-grammars` wheels. Every pull request and push explicitly
prefetches the required grammars into a cached build directory, builds the wheel,
verifies every checksum, parses one fixture per grammar plus embedded Svelte/Vue/Astro/Liquid
regions in an isolated environment, and uploads the artifact. Dispatch release builds may
add an OIDC Sigstore signature. Runtime analysis never downloads grammars silently: the
installed companion is activated once before parser workers start. `dev map doctor`
reports `35/35` when the wheel is complete, otherwise it lists missing primary and embedded
grammars and tells the user to install the matching platform wheel.

## Debugger and runtime behavior

```bash
dev debug discover --consent
dev debug start --adapter debugpy --config-json '{"program":"app.py"}'
dev debug break SESSION app.py 12
dev debug stack SESSION --thread 1
dev debug evaluate SESSION 'expression' --frame 2 --allow-side-effects
dev debug trace --python-script app.py
dev debug trace --import node.cpuprofile
dev debug stop SESSION
```

Debugger CLI sessions live in a token-protected loopback broker so control survives separate CLI invocations. MCP owns sessions in-process. DAP controls execution and inspects stopped state; exact/sampled runtime evidence is deliberately separate: Python uses `sys.setprofile`, Node consumes CPU profiles, DAP stacks are sampled observations, and JSONL providers can be imported. `evaluate` is a separate side-effectful operation and requires explicit approval. Debug values are truncated and secret-redacted.

Runtime edges never become timeless static facts. Every session records repository/dirty-tree, build/configuration, adapter executable, and provider fingerprints; observations contribute to liveness and paths only when the source fingerprint matches the current workspace.

## Dead-code confidence tiers

| Tier | Meaning |
| :--- | :--- |
| `extracted` | No inbound call/import edges in the resolved graph **and** token-scan agrees |
| `inferred` | Only callers are themselves dead (transitive island), or methods with no inbound calls |
| `ambiguous` | Graph-dead but token-scan or name-only refs suggest a possible false positive |

Full and incremental builds enrich framework semantics before liveness. Unambiguous
`routes_to`, `listens`, and `provides` bindings make their handler/provider target live
even when the framework invokes it without an ordinary source-level call. Ambiguous
name matches remain unresolved and never suppress a dead-code candidate.
Routing, DI, and event matchers are isolated behind the framework manifest and covered
by one fixture per advertised family. Imported aliases and bounded callback/type aliases
may resolve a target, but multiple candidates remain ambiguous. Liveness follows the
registration owner through the registration node to its target; a registration inside a
dead setup function does not make a route, provider, or observer callback live.

Prefer `dev map dead --confidence extracted` plus file greps before deleting anything.
Treat `inferred` as **unconfirmed**. If `entry_roots` are empty or
`liveness_unreachable_unreliable` is set, **ignore** `unreachable_files` and mass inferred dead.

`dev map` stores a **capped** (5000) `dead_symbol_candidates` list for agents:
**extracted ∩ token-scan** (methods excluded). `dev map dead` reports the **uncapped**
graph tiers with reasons. Prefer reviewing `ambiguous` before deleting anything. If graph
assemble fails, the map omits dead-symbol candidates rather than falling back to a token-only flood.

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
3. Prefer `dev map dead --confidence extracted` + greps; treat inferred as unconfirmed.
   If entry roots are empty / unreliable, ignore unreachable and mass inferred dead.
   Check `unwired_candidates` / `dead_symbol_candidates` before adding modules.
4. Use `dev map query` / `trace` / `dead` for symbol-level navigation.
5. Run `dev map` after large refactors (or rely on hooks / `--watch`).

## API route mapping

Native HTTP surface tools over `ROUTE` nodes and `routes_to` /
`registers` edges (no external graph service):

```bash
dev map routes --json
dev map shape-check --json
dev map shape-check --route /api/items --json
dev map api-impact /api/items --json
```

- **`routes`** — handlers, registration owners, and client fetch sites
  (`fetch`, `axios`, `requests`, `httpx`) matched by normalized path
  (`:param`, `{id}`, `[id]` → `*`).
- **`shape-check`** — handler return dict keys (Python AST / TS regex) vs keys
  accessed on the response variable in a short post-fetch window.
- **`api-impact`** — consumers, middleware registrations, shape mismatches, and
  a risk tier (`high` / `medium` / `low` / `none`).

MCP equivalents: `devcouncil_route_map`, `devcouncil_shape_check`,
`devcouncil_api_impact`, `devcouncil_graph_ingest`, `devcouncil_graph_cypher`,
`devcouncil_pdg_query`, `devcouncil_explain`.

## Corpus side index

For navigation over docs, PDFs, and images — separate from the
deterministic code graph but able to feed opt-in verify gates — see
[docs/corpus.md](corpus.md) and run `dev corpus build`, `dev corpus query`, and
`dev corpus status`.
