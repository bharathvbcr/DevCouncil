# Repository Map & Code Graph

DevCouncil builds a deterministic repository map and a symbol-level knowledge graph without calling an LLM. Agents use these artifacts to navigate, find callers, and spot unwired or dead code before inventing new modules.

## Artifacts

| Path | Role |
| :--- | :--- |
| `.devcouncil/repo_map.json` | File inventory, subsystems, entry roots, unwired/unreachable/dead-symbol candidate lists, reverse-import dependents |
| `.devcouncil/graph/code_graph.json` | Symbol nodes + edges (imports, named imports, calls, inherits, contains) and tiered `dead_code` |
| `.devcouncil/codeintel/index.sqlite` | Canonical WAL-mode graph, source cache, FTS, generations, unresolved references, diagnostics, and fingerprinted runtime observations |
| `.devcouncil/graph/graph.html` | Self-contained interactive visualizer (`dev graph html`; **not** written by default on `dev map`) |
| `AGENTS.md` / `CLAUDE.md` | Marker-guarded workspace guides kept in sync with the map |

## Build / refresh

```bash
dev map                     # Full rebuild (liveness on by default)
dev map --if-stale          # No-op when fingerprints still match
dev map --no-liveness       # Skip entry/unwired/unreachable/dead lists
dev map --lsp-refs          # Confirm dead symbols via live LSP references
dev map --wiki / --no-wiki  # Refresh codebase-wiki skeletons after map (on by default)
dev map --scan-deps         # Opt-in SCA (pip-audit / npm audit / osv-scanner) → dependency_risks
dev map --watch             # Debounced incremental refresh on edits
dev graph init              # Build canonical SQLite + compatibility exports
dev graph ingest            # Unified analyze: codeintel sync → graph export → repo map write
dev graph ingest src/foo    # Path-scoped ingest (full reconcile when paths omitted)
dev graph status            # Generation, pending paths, watcher/degraded state
dev graph sync              # Reconcile and commit now
dev graph watch             # Native FSEvents/inotify/ReadDirectoryChangesW foreground watcher
dev graph doctor            # SQLite, watcher, and offline grammar verification
```

Freshness uses git HEAD, a tracked-file hash, and a content fingerprint so plain edits mark the map stale. Fingerprint / git errors fail closed (treat as stale). A **missing** `.devcouncil/repo_map.json` is also stale — hard rigor blocks checkout/verify until `dev map` or `dev graph ingest` runs. Post-tool-use hooks and `dev map --watch` refresh incrementally; incremental extract still verifies parse-cache sha256 so a concurrent edit to an unlisted path cannot stamp a fresh fingerprint over stale symbols.

HTML visualizer: set `indexing.write_graph_html: true` in config if you want `dev map` to also write `graph.html`. Otherwise use `dev graph html` / `dev graph view` explicitly.

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
dev graph pdg build --path src/foo/bar.py        # on-demand for paths
dev graph explain --category command-injection
dev graph pdg-query --mode controls --target my_fn
dev graph pdg-query --mode flows --target my_fn --variable x
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
dev graph query build_code_graph   # definition + callers/callees/importers
dev graph trace path/a.py path/b.py
dev graph dead                     # full dead-code report (uncapped)
dev graph dead --min-confidence inferred
dev graph check                    # god nodes + circular imports
dev graph process                  # BFS call-flows from entry roots
dev graph impact src/foo.py        # blast radius
dev graph impact --diff            # blast radius for working-tree changes
dev graph html                     # write graph.html
dev graph view                     # serve/open the HTML
dev graph export -o out.graphml    # GraphML (or --format okf)
dev graph search request_handler   # FTS5 symbol/path search
dev graph search "auth flow" --semantic  # Opt-in local embeddings (indexing.embeddings.enabled)
dev graph cypher 'MATCH (a)-[r:CALLS]->(b) RETURN a.id, b.id LIMIT 20'
dev graph explore request_handler  # source + semantic paths + blast radius
dev graph affected src/foo.py      # tests in the inbound impact closure
```

## Transactional code intelligence

SQLite is canonical; graph v2 JSON remains a deterministic compatibility export. A refresh writes a complete generation in one transaction and advances the current-generation pointer only after every file, node, edge, liveness record, and FTS row is committed. Readers therefore see the complete previous or complete next graph. The store retains two committed generations for rollback/debugging and caches compressed source and extraction facts by content, grammar, analyzer, and configuration hashes.

MCP starts one project watcher for its server lifespan. Queries wait up to two seconds for a pending batch; if syncing cannot finish, responses retain the last committed generation and identify pending/degraded state. The same Git-aware scope filter handles the initial index, FSEvents/inotify/ReadDirectoryChangesW events, reconciliation, ignored directories, atomic saves, and deletes. Incremental sync replaces changed files plus their reverse-import closure, unresolved-reference candidates, and matching framework registries; unaffected static edges are copied forward without re-resolution. New subsystem topology or a scope above 20% of indexed code files deliberately falls back to a clean rebuild.

The 35-language grammar matrix is delivered through platform-specific
`devcouncil-codeintel-grammars` wheels. Every pull request and push explicitly
prefetches the required grammars into a cached build directory, builds the wheel,
verifies every checksum, parses one fixture per grammar plus embedded Svelte/Vue/Astro/Liquid
regions in an isolated environment, and uploads the artifact. Dispatch release builds may
add an OIDC Sigstore signature. Runtime analysis never downloads grammars silently: the
installed companion is activated once before parser workers start. `dev graph doctor`
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

Prefer `dev graph dead --confidence extracted` plus file greps before deleting anything.
Treat `inferred` as **unconfirmed**. If `entry_roots` are empty or
`liveness_unreachable_unreliable` is set, **ignore** `unreachable_files` and mass inferred dead.

`dev map` stores a **capped** (5000) `dead_symbol_candidates` list for agents:
**extracted ∩ token-scan** (methods excluded). `dev graph dead` reports the **uncapped**
graph tiers with reasons. Prefer reviewing `ambiguous` before deleting anything. If graph
assemble fails, the map omits dead-symbol candidates rather than falling back to a token-only flood.

Map liveness lists (`unwired_candidates`, `unreachable_files`, `dead_symbol_candidates`) are
capped at **5000** each; `dependents[path]` is capped at **256** per file. When a list hits
its cap, metadata records `*_truncated` plus totals — use `dev graph dead` for uncapped tiers.

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
3. Prefer `dev graph dead --confidence extracted` + greps; treat inferred as unconfirmed.
   If entry roots are empty / unreliable, ignore unreachable and mass inferred dead.
   Check `unwired_candidates` / `dead_symbol_candidates` before adding modules.
4. Use `dev graph query` / `trace` / `dead` for symbol-level navigation.
5. Run `dev map` after large refactors (or rely on hooks / `--watch`).

## API route mapping

Native HTTP surface tools over `ROUTE` nodes and `routes_to` /
`registers` edges (no external graph service):

```bash
dev graph routes --json
dev graph shape-check --json
dev graph shape-check --route /api/items --json
dev graph api-impact /api/items --json
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
