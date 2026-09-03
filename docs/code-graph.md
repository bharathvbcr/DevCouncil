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
| `.devcouncil/graph/code_graph.json` | Compact export of symbol nodes + edges (imports, named imports, calls, inherits, contains) and tiered `dead_code`, written by the kernel from the same generation as the map. **The kernel store is canonical**; prefer `dev map query` / `trace` / `dead` when the JSON is missing. |
| `.devcouncil/codeintel/devmap.sqlite` | **Canonical.** The Rust kernel's WAL-mode store: generations, nodes, edges, unresolved references, FTS5, the pending-path queue, build history. Written only by `devmap build` (every `dev map`, `init`, `ingest`, `sync`, verify/checkout refresh and MCP `devcouncil_graph_ingest` go through it). |
| `.devcouncil/codeintel/index.sqlite` | The Python query cache. Not an engine: `load_code_graph` imports the kernel's `code_graph.json` into it on first read after a build, and the Python-only commands (`check`, `process`, `routes`, `cypher`, `pdg`, …) answer from that cache. Safe to delete; it is rebuilt from the JSON. |
| `.devcouncil/graph/graph.html` | Self-contained interactive visualizer (`dev map graph-html` / `dev map html --symbols` / alias `dev graph html`; **not** written by default on bare `dev map`) |
| `.devcouncil/map.html` | Self-contained subsystem map visualizer (`dev map html`; slim payload — no `files[]` / `dependents{}`) |
| `.devcouncil/graph/demo.html` | Sample self-contained interactive UI from `dev map demo` (no map required; primary demo artifact) |
| `.devcouncil/graph/demo.svg` | Optional static companion written by `dev map demo` (not the interactive UI) |
| `AGENTS.md` / `CLAUDE.md` | Marker-guarded workspace guides kept in sync with the map |

## Build / refresh

```bash
dev map                     # Build through the kernel: no-op on an unchanged tree, incremental otherwise
dev map --full              # Force a cold rebuild in the kernel
dev map --goal "…"          # Rank candidate_files for a goal (ripgrep-based, layered on the kernel's map)
dev map --if-stale          # Exit 0 without building when fingerprints still match; never starts a cold build
dev map --wiki / --no-wiki  # Refresh codebase-wiki skeletons after map (on by default)
dev map --scan-deps         # Opt-in SCA (pip-audit / npm audit / osv-scanner) → dependency_risks
dev map --pdg               # Opt-in Python PDG/CFG/taint layer over the kernel's graph
dev map --watch             # Event-driven rebuild on edits (same as `dev map watch`)
dev map html                # Write interactive .devcouncil/map.html (subsystems)
dev map html --open         # Write and open the subsystem map
dev map graph-html          # Write symbol-level .devcouncil/graph/graph.html
dev map html --symbols      # Same as graph-html
dev map init                # Same build as `dev map`, JSON-friendly report (`--json`)
dev map ingest [paths]      # Same build; paths are reported back, the kernel decides incrementality
dev map sync                # Same build
dev map status              # Engine binary, store (schema, size, free pages, WAL), kernel freshness, daemon, artifacts
dev map doctor              # Verdicts with fixes: kernel present/capable, store no newer than kernel, artifacts kernel-written, reclaim/WAL pressure
dev map repair --pending    # Drop pending-queue entries the kernel can never index
dev map unlock              # Free a stuck *legacy* Python query-cache lease (the kernel's lock is released on process death)
```

`--no-liveness` and `--lsp-refs` are gone: the kernel always computes liveness, and the LSP adjunct was cut with the Python engine. A flag that is accepted and ignored is worse than one that is rejected, so both are rejected.

### One writer

The kernel is the only writer of `repo_map.json` and `code_graph.json`. `dev map`, `dev map init` / `ingest` / `sync`, the verify-time and checkout-time refresh, `dev plan`, `dev init`, and MCP `devcouncil_graph_ingest` all call `refresh_map_artifacts`, which runs `devmap build` + `devmap manifest` and then layers on what the kernel does not do: goal ranking, dependency auditing, the marker-guarded agent guides (`AGENTS.md` / `CLAUDE.md`), the wiki skeletons. Before 2026-09-02 those callers ran the retired Python engine into `index.sqlite` and rewrote the map from a generation built from an old HEAD — two writers, one artifact, and the last one to run won.

### When a map looks wrong — for a person or an agent

Every kernel run the seam launches (`build`, `manifest`, `repair`) is recorded in the project trace log (`.devcouncil/logs/traces.jsonl`, event type `devmap_run`) with its argv, exit code, duration, the kernel's notes (discovery refusals, reclaim, progress) and, on failure, a diagnosis code. A failing `dev map` prints `[code]`, the fix, and the run id. While a build runs, `.devcouncil/codeintel/devmap-build.live.json` carries its pid and latest progress line so another process can see it.

1. **`dev map status`** (`--json`) — which binary will run and when it was built, the store's schema against the binary's, free-page ratio and WAL size, the kernel's own `is_fresh` / pending / quarantined counts, whether a daemon holds the socket, who wrote each artifact, whether a build is **running** (pid, stage, elapsed, seconds since its last progress line) or left a marker behind, and the **last build** with its run id.
2. **`dev map doctor`** (`--json`) — the same facts as verdicts. Every check carries `code` (what an agent branches on), `fix` (a sentence) and `fix_command` (what it runs). Critical (exit 1): `engine_missing`, `schema_newer_than_kernel` (rebuild it: `cargo build --release -p devmap-cli`, or point `DEVMAP_BINARY` at a newer build), `foreign_writer`, `store_unreadable`. Warnings: `reclaim_pressure`, `wal_large`, `stale_map`, `pending_paths`, `stale_build_marker`, `build_stuck` (no progress for 10 min while the pid lives), `last_build_failed:<code>`.
3. **`dev map doctor --fix`** — applies every fix a repository can apply and re-checks: clears a dead build's marker, quarantines an unreadable store (kept as `devmap.sqlite.corrupt-<stamp>`), drops stuck queue rows, and runs **one** build for everything a build resolves. It never touches a running build and never rebuilds the kernel binary; those are listed under `not_applied` with their commands.
4. **`dev map runs [--last N] [--failed] [--json]`** — the records. The run id from a failure is the key.
5. **`dev map abort`** — stops the build the live marker names (SIGTERM, then SIGKILL after five seconds). Safe: a generation is one transaction, so the store stays on the prior generation and the OS releases the writer lock. Refuses a pid that is not a devmap process.
6. **`dev map --full`** — when the store should be rebuilt from nothing.

The same surfaces exist over MCP: `devcouncil_graph_doctor` (`fix: true` applies), `devcouncil_graph_runs` (`limit`, `failedOnly`), and `devcouncil_graph_ingest` returns the diagnosis (`kernel_code`, `fix`, `run_id`, `stage`) alongside `engine_unavailable` when a build fails. `devcouncil_tail_trace` shows the run records among the other trace events.

Failure codes a build can raise: `binary_missing`, `schema_newer_than_kernel`, `store_locked` (another writer holds the store; status shows it, `dev map abort` if it is stuck), `store_corrupt`, `store_unwritable`, `kernel_flag_unsupported` (the binary predates a flag the seam passes), `kernel_timeout` (the record carries the last progress line), `kernel_failed` (anything else, with the kernel's last lines as evidence).

Freshness uses git HEAD, a tracked-file hash, and a content fingerprint so plain edits mark the map stale. Fingerprint / git errors fail closed (treat as stale). A **missing** `.devcouncil/repo_map.json` is also stale — hard rigor blocks checkout/verify until `dev map` runs. The guides a build writes are restamped into the fingerprint, so a build never makes its own map read stale. Post-tool-use hooks run `dev map --if-stale --no-wiki`; `dev map --watch` wakes on filesystem events (debounced, with a slow poll as the safety net) and checks exactly the fingerprint `--if-stale` reads.

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
dev map query refresh_map_artifacts  # definition + callers/callees/importers
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
dev map search request_handler     # FTS5 symbol/path search (kernel)
dev map search "auth flow" --semantic  # Name-similarity ranking in the kernel; no embedding index
dev map cypher 'MATCH (a)-[r:CALLS]->(b) RETURN a.id, b.id LIMIT 20'
dev map explore request_handler    # source + semantic paths + blast radius
dev map affected src/foo.py        # tests in the inbound impact closure
```

## How a build works

`devmap build` extracts every source file with tree-sitter (30 linked grammars; grammarless declaration languages such as `.proto` and `.ps1` get pattern-recovered symbols stamped as fallback; prose and data formats contribute a `File` node and are not counted as parse failures), resolves imports and calls across the whole tree, runs liveness and community detection, and persists one generation in a single transaction. An unchanged tree (same content hashes, same kernel) returns without writing; a changed one carries unaffected files forward and rewrites the affected closure. The store keeps two generations, prunes the extraction cache to what they reference, and reclaims free pages with an incremental vacuum followed by a WAL checkpoint — on the unchanged path too, so a quiet repository does not sit at 40% free pages.

Concurrent writers (`dev map` in two shells, a daemon drain and a build) serialise on an advisory file lock next to the store with a bounded wait; the loser fails with the holder's pid rather than a bare `database is locked`, and the lock is released by the operating system when the holder dies.

The daemon (`devmap serve`, started on demand by the Python client for queries) watches the tree, coalesces events, and drains pending paths into the same store. Paths it cannot process — outside the root, no longer existing, oversized, not a source file — are dropped, not retried forever; `dev map status` names any that remain quarantined and `dev map repair --pending` drops them. The daemon retires after 30 idle minutes, when its executable changes, on SIGTERM, and when its root or store disappears.

### File inventory (Python side)

The freshness fingerprint and the goal ranking are computed over the git inventory (`RepoMapper.get_git_files`):

- `indexing.include_untracked` (default **on**) — include untracked-but-not-ignored files. **Keep this on**: a file an agent just wrote and has not staged is otherwise absent from the fingerprint, so a build after that write reads fresh.
- `indexing.max_indexed_files` (default 50000) — hard ceiling on the inventory. Untracked paths are dropped first and the overflow is logged, never silently truncated. Generated trees (`node_modules`, `target`, `coverage`, `vendor`, `Pods`, `.next`, `.devcouncil`, binaries, archives, …) are excluded at any depth regardless.

## Rust engine (`devmap`)

The `dev map` engine is **`devmap`**, a seven-crate workspace under [`rust-port/`](../rust-port/):

| Crate | Responsibility |
| :--- | :--- |
| `devmap-extract` | tree-sitter parsing, wiring annotations, framework matchers, explicit unavailable outcomes |
| `devmap-resolve` | import, call, and receiver-type resolution with typed confidence |
| `devmap-analyze` | liveness / dead-code tiers and weighted communities |
| `devmap-store` | rusqlite v6 schema, pending queue, history, differential writes |
| `devmap-query` | token-budgeted search / deps / manifest |
| `devmap-serve` | file watcher and durable pending drain |
| `devmap-cli` | the `devmap` binary |

**The kernel is the production map engine; there is no Python fallback for building.** The
Python seam is two files: `src/devcouncil/devmap_engine.py` runs `devmap build` and
`devmap manifest` and stamps freshness, and `src/devcouncil/devmap_client.py` speaks the
newline-framed JSON IPC to a `devmap serve` daemon (one per repository, socket derived from the
canonical root; spawned on demand, never by a status probe, and never when `DEVMAP_AUTOSPAWN=0`)
with a CLI fallback for every request. The kernel binary is located by one rule for both:
`DEVMAP_BINARY` if set, else the newest capable build among `<repo>/rust-port/target/{release,debug}`,
`<package>/rust-port/target/{release,debug}` and `PATH` — "capable" being what `manifest --help`
advertises, because every build reports the same version string.

Query surfaces that still run on the Python side (`check`, `process`, `routes`, `shape-check`,
`api-impact`, `cypher`, `explore`, `affected`, `pdg`, the HTML visualizers) read the Python
query cache, which `load_code_graph` fills from the kernel's `code_graph.json` after each build.

```bash
cd rust-port && ./verify.sh
cargo build --release -p devmap-cli
./target/release/devmap --version                       # devmap 0.1.0 (schema N)
./target/release/devmap --db /tmp/t.sqlite --progress always build ./testdata
./target/release/devmap --db /tmp/t.sqlite search helper --budget 500
```

Known limits, kept explicit rather than papered over:

- **Grammar coverage is 5 of 35.** Only Python, JavaScript, TypeScript/TSX, Rust, and Go are
  linked. Every other registry language returns `ParseOutcome::Failed` — a failed parse is never
  promoted to fabricated clean output, and failed outcomes are not cache-admitted.
- **No parity sign-off.** The parity harness still reports diffs against the Python-derived
  goldens; local passing tests are mechanical evidence only, not a shadow soak or production run.
- **No cutover, no deletion, no publication.** Phase 6 consumer migration is the active work.

[rust-port/STATUS.md](../rust-port/STATUS.md) is the authoritative ledger of what is verified and
what is open; [rust-port/PHASE1_CONTRACT.md](../rust-port/PHASE1_CONTRACT.md) freezes the
35-language specification the Rust registry is written against.

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

Framework-invoked symbols are kept live by the kernel, which resolves them during the
build rather than in a Python enrichment pass afterwards. Two mechanisms do it. Edges:
an unambiguous `routes_to` (kernel `EdgeKind::HandlesRoute`) or `subscribes`
(`SubscribesTo`) makes its handler target live even though nothing calls it from source.
Exemptions: `WiringKind` marks a symbol the runtime invokes with no observable call site
at all — `FrameworkDecorator`, `RuntimeEntryPoint` (`func init`, `#[test]`,
`componentDidMount`, `pytest_*`), `ScriptEntry`, `Launcher`, `ReExportPackage`,
`StructuralExempt` (a Rust trait-impl method cannot carry `pub`, so `is_exported` says
nothing about it) — and an exempt symbol reports the reason it was exempted, per symbol
or per file. Ambiguous name matches stay unresolved and never suppress a dead-code
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
