# Consumer Contracts Ledger (Task 1.4)

This document tracks all 37 external consumer files in `src/devcouncil/` that interface with code intelligence (`devcouncil.indexing` / `devcouncil.codeintel`), their touched surfaces, expected payload shapes, and tri-state expectations (X14: unknown ≠ verified-zero).

## Correction (2026-09-02, evening): one build path, no Python writers

The "hybrid" column below is historical. Since the kernel audit's second pass:

- **Every write of `repo_map.json` / `code_graph.json` goes through
  `devcouncil.indexing.map_artifacts.refresh_map_artifacts`**, which runs
  `devmap build` + `devmap manifest`. Callers: `dev map`, `dev map init|ingest|sync`,
  `dev init`, `dev plan`, the verify-time and checkout-time refresh
  (`indexing.map_refresh.refresh_stale_map_if_needed`), the post-tool-use hook
  (`cli/commands/hook.py`), MCP `devcouncil_graph_ingest` and the MCP
  `codeintel sync` tool. There is no Python fallback; a build that cannot run
  raises `DevMapEngineError` carrying `code` / `fix` / `run_id`.
- **Deleted:** `codeintel/sync/{coordinator,incremental,scope}.py`,
  `codeintel/build_worker.py`, `build_control.run_isolated_full_build` and its
  worker helpers, `indexing.graph.build.refresh_map_for_paths`,
  `indexing.map_refresh.refresh_repo_map_from_graph`. `codeintel/sync/lease.py`
  and `build_control.graph_build_session` stay: they guard the Python
  **query cache** (`index.sqlite`), which `load_code_graph` fills from the
  kernel's `code_graph.json` and which `check` / `process` / `routes` /
  `cypher` / `pdg` / the HTML visualizers still read.
- **The MCP server no longer runs a watcher of its own**; its lifespan warms the
  kernel daemon (`devmap serve`, spawned by `DevMapClient` on demand, retiring
  itself when idle).
- **New consumer-facing surfaces:** `dev map status|doctor|runs|abort`,
  `dev map doctor --fix`, MCP `devcouncil_graph_doctor` and
  `devcouncil_graph_runs`; the run records are `devmap_run` events in
  `.devcouncil/logs/traces.jsonl`. Contract in `docs/code-graph.md` → "When a
  map looks wrong".
- **Binary discovery is one rule** for the seam and the client
  (`devmap_engine.find_engine_binary`): `DEVMAP_BINARY` (used or refused by
  name, never replaced), else the newest
  *capable* build in `<repo>/rust-port/target`, `<package>/rust-port/target`,
  `PATH`. **Socket path is one formula** (`devmap_client.default_socket_path`
  mirrors `devmap-serve::ipc_identity_for`; `devmap serve --print-socket-path`
  is the oracle).

The state of the Python-side retirement at the moment this was written is in
`AGENT_PLAN.md` → "Handoff (2026-09-02, evening)".

## Correction (2026-08-17): the "hybrid" consumers were Python-only in practice

The table below described seven consumers as Rust-primary with a Python
fallback. That was true of the *code* and false of the *behaviour*: until SC23,
`DevMapClient.DEFAULT_DB_PATH` pointed at `.devcouncil/codeintel/index.sqlite`,
which is the Python schema (`user_version = 2`). The Rust store is `v12` (was `v10` when this
was written; `CURRENT_SCHEMA_VERSION` in `devmap-store/src/schema.rs` is authoritative) and
fails closed on it, so every hybrid call raised `DevMapClientError` and took the
fallback — on every invocation, in this repository, since the pass landed.

Two changes make the primary path real, and both are verified rather than
asserted:

- **SC23** points the client at the Rust kernel's own store,
  `.devcouncil/codeintel/devmap.sqlite`, so the two schemas coexist during
  cutover. After `devmap build`, `DevMapClient(root).status()` returns
  generation 7 with 12,103 nodes and `impact()` returns 80 items with
  `resolution == "Available"` — the Rust surface answering a Python consumer for
  the first time here.
- **SC24** stops `try_connect` accepting a store that has nothing to say. A
  missing database is *created empty* by `devmap status` and a build over a
  source-less directory commits a zero-node generation; both used to read as
  "available" and returned a confident zero. Availability now requires a
  committed generation and a non-zero node count.

Consequence for the table: **"Hybrid" now means the Rust path genuinely runs
when a built store is present.** Rows below that were never re-verified against
a live Rust store should be treated as unproven until they are.

## Migration status (2026-08-12, Phase 6 consumer pass)

| Status | Consumers |
|--------|-----------|
| **DevMapClient-primary** | None yet (Python index schema ≠ Rust store; live repo still needs Python fallback). |
| **Hybrid (client + Python fallback)** | `integrations/mcp/handlers/codeintel.py` (search/path/impact/dead/status/sync-build); `integrations/mcp/handlers/map.py` (stale/symbols/dead/impact enrichment); `integrations/mcp/util.py` (`with_codeintel_freshness`); `cli/commands/graph_cmd.py` (search/query/trace/dead/impact/status); `verification/checks/dead_symbols.py`; `verification/checks/stale_map.py`; `verification/checks/wiring.py` (impact enrichment). |
| **Pending (priority leftovers)** | `verification/checks/orphan_diff.py` (path heuristic only); `subsystem_boundary.py` (repo_map JSON helpers); `semantic_diff.py` (SemanticIndex ≠ snapshots); `acceptance_corpus.py` (corpus extract); `verify_orchestration.py` / `verifier.py` (liveness snapshot / map refresh); MCP adjuncts (`ast_lsp`, `debug`); remaining CLI/reporting/execution consumers. |
| **Frozen (R1)** | `cli/commands/map.py` — not modified in Phase 6 thin-client track |

Client APIs added this pass: `try_connect()`, `resolution_unavailable_reason()` (transport/tri-state helpers only).

Grep proof for remaining direct imports (excluding `indexing/` + `codeintel/` trees):

```bash
rg -l 'from devcouncil\.(indexing|codeintel)' src/devcouncil --glob '*.py' \
  | rg -v '^src/devcouncil/indexing/' \
  | rg -v '^src/devcouncil/codeintel/' | wc -l
# 37 (2026-08-12; hybrid consumers still import Python fallbacks)
```

Hybrid consumers (DevMapClient/`try_connect` present): **7** files under `src/devcouncil/` excluding the client itself.

## Correction (2026-09-02): three surfaces this ledger does not track, and one freeze it records as intact

**The matrix has exactly 37 rows and the seam is not one of them.** `src/devcouncil/devmap_engine.py`
is what every `dev map` build now executes — it invokes `target/release/devmap` and is the
boundary between the Python CLI and the Rust kernel. Neither it nor `devmap_client.py` nor
`indexing/graph/embeddings.py` appears here. The ledger's stated scope is files that interface
with code intelligence; on that reading the most load-bearing interface in the system is
untracked. Whether the seam belongs in this matrix or in a section of its own is a real
question — it consumes the *Rust* kernel rather than the Python modules the matrix was built
around — but "absent" is not the right answer either way.

**`cli/commands/map.py` is recorded above as `Frozen (R1) — not modified in Phase 6 thin-client
track`. It was modified on 2026-09-02**, gaining a `_refresh_embeddings()` call in `_build_once`
so the embedding index is rebuilt with the map. The change is tested and works; that is not the
point. R1 froze this file, this ledger records the freeze, and STATUS.md required-decision 3
says changes to frozen Python consumers need approval before they land. The edit is in the
working tree awaiting that decision, and this row should not be read as still accurate.

Two further rows changed on the same date and are less consequential, but were not re-verified
against their contracts:

| Row | File | Change |
|---|---|---|
| 15 | `integrations/mcp/util.py` | `TextContent` moved to `TYPE_CHECKING` with a local import in `json_text()`; response shape unchanged |
| — | `indexing/graph/embeddings.py` | **Deleted in `079bc50`.** The `K7` ranker fix landed here first as a stored TF-IDF index (IDF table, build step, generation stamp, model tag, `stale_rows_skipped`); the kernel now does the same ranking statelessly per query in `devmap-query/src/semantic.rs`, so none of that machinery survives. `EmbeddingsConfig` went with it in `6a4e656`, along with the init-template entry and three build call sites; `grep -r 'EmbeddingsConfig\|hash-v1' src/` is empty. (`SemanticCacheConfig` in `app/config.py` is a different, live subsystem — the FAISS cache for the LLM layer — and is not related to graph embeddings) |

That row read differently when it was written, and the rewrite is the point: it described the
embedding change as *"a behaviour change to a stored artifact, not just an algorithm swap"*,
with old-model rows skipped until rebuilt. Within hours the stored artifact was removed
entirely. Nothing is stored now, so there is no stale row to skip and no coverage subset to
warn about — the warning outlived the thing it warned about, which is the failure this ledger
exists to prevent.

## Correction (2026-09-10): what may be called unwired, and the counters that say what was not

**One predicate now decides whether a file can be the subject of a file-level liveness
verdict at all.** `Extraction::file_liveness()` in `devmap-extract/src/model.rs` answers
`NotCode { reason }`, `Exempt { kind, reason }` or `Candidate`, and
`unwired_candidates`, `files_wholly_inside_clusters` (which produces `unreachable_files`)
and `analyze_liveness`'s file exemption all ask it. They decided separately before, and
the first of them decided by asking `grammar_read_this_file()` — a question about *which
engine ran*. Prose stayed out of `unwired_candidates` only because no grammar is linked
for Markdown or YAML; linking one would have made every `.md`, `.json` and `.yaml` in
every repository a delete-this suggestion again.

**Nothing was removed from either artifact.** These keys are additive, and every
pre-existing counter keeps its name and its meaning:

| Artifact | Key | Meaning |
|---|---|---|
| `repo_map.json` | `liveness_meta.unwired.excluded_not_code` | Files that were never candidates: prose, data, configuration, lockfiles, environment files |
| `repo_map.json` | `liveness_meta.unwired.excluded_not_code_reasons` | The same count, keyed by reason. "412 data files" and "412 lockfiles" are different problems |
| `repo_map.json` | `liveness_meta.unwired.excluded_exempt` | Code reached by something no import edge records: an entry root, a test, a fixture, a package marker, a tool config, an ambient declaration |
| `repo_map.json` | `liveness_meta.unwired.excluded_directory_unit` | A **subset** of `excluded_exempt` — a `.tf` file, whose unit of use is its directory. Never add the two |
| `code_graph.json` | `meta.devmap_rust.unwired_excluded_not_code` (+ `_reasons`, `_exempt`, `_directory_unit`) | The same four, under the key `code_graph.json` already uses for a producer's account of its own run |
| `code_graph.json` | `nodes[].extras.liveness` | `"not_applicable" \| "exempt" \| "candidate"`, on **file** nodes only |
| `code_graph.json` | `nodes[].extras.liveness_reason` | Why, for the first two. Absent on a candidate |

**A consumer that predates these keys is unaffected.** `RepoMap` in `repo_mapper.py`
declares `liveness_meta` as a dict and pydantic ignores unknown keys, so no
`model_validate` call site changes; `GraphNode.extras` is already a free-form map, which
is why the liveness verdict went there rather than onto the node as a top-level field.
GitPulse's `RepoMapUnwiredMeta` (`src/lib/codeintel/types.ts`) declares all four as
**optional** for the same reason, and its panel renders an absent counter as no phrase
rather than as zero — a map from an older kernel did not look, which is not the same fact
as a kernel that looked and found none.

**What changed in `entry_roots`.** `*.config.*` used to arrive as a `TargetRoot` and now
arrives as a `ToolConfig`, which `is_entry_root` also accepts, so those files are still
entry roots. `*.d.ts` used to arrive as a `TargetRoot` too and now arrives as an
`AmbientDeclaration`, which `is_entry_root` does **not** accept: a declaration file is
consulted by the compiler and nothing runs it, and `subsystem_map.is_entry_root` passes
that list on to a reader as "this is where execution starts". A repository's
`entry_roots` therefore shrinks by its `.d.ts` count. The files remain exempt from every
liveness verdict.

## The 37 Consumers Matrix

| # | File | Touched Surface / API | Expected Payload Shape | Tri-State / Cutover Notes |
|---|------|-----------------------|------------------------|---------------------------|
| 1 | `src/devcouncil/reporting/okf_bundle_writer.py` | `repo_map.json` / `code_graph.json` | JSON dict with `subsystems`, `files` | Tri-state `resolution`: unavailable when unindexed |
| 2 | `src/devcouncil/cli/commands/plan.py` | `repo_map.json` | JSON dict | Tri-state: missing map treated as unindexed |
| 3 | `src/devcouncil/cli/commands/graph_cmd.py` | CLI graph query API | JSON / dict graph result | Cutover: socket client delegate |
| 4 | `src/devcouncil/cli/commands/semantic.py` | Semantic index query | Dict list of symbols | Cutover: socket client query |
| 5 | `src/devcouncil/cli/commands/hook.py` | `stale_map` check | Status code / boolean | Cutover: daemon socket status |
| 6 | `src/devcouncil/cli/commands/map.py` | CLI map trigger | Status report / JSON | Cutover: daemon RPC invocation |
| 7 | `src/devcouncil/cli/commands/lsp.py` | LSP client API | Diagnostics / symbol definitions | Adjunct port in Phase 7 |
| 8 | `src/devcouncil/cli/commands/debug_cmd.py` | Debug tracing API | Trace event stream | Adjunct port in Phase 7 |
| 9 | `src/devcouncil/cli/commands/ast.py` | **PRIVATE INTERNAL REACH**: `devcouncil.indexing.ast_matcher` | AST match payload | **WILL BREAK AT CUTOVER** — replace with supported socket API |
| 10 | `src/devcouncil/cli/commands/init.py` | Initial graph generation | Status code | Cutover: daemon initialization |
| 11 | `src/devcouncil/cli/commands/wiki.py` | Wiki refresh / community map | Markdown / HTML wiki pages | Cutover: daemon query API |
| 12 | `src/devcouncil/cli/commands/doctor.py` | Codeintel health check | Status dict | Cutover: `devmap serve` ping |
| 13 | `src/devcouncil/integrations/claude_assets.py` | Manifest generator | Text manifest (≤2k tokens) | Cutover: `Budgeted<T>` manifest surface |
| 14 | `src/devcouncil/integrations/mcp/server.py` | MCP tool routing | Tool response dicts | Cutover: thin client delegation |
| 15 | `src/devcouncil/integrations/mcp/util.py` | Graph/symbol helper formatting | JSON responses | Cutover: thin client formatters |
| 16 | `src/devcouncil/integrations/mcp/handlers/codeintel.py` | Codeintel search/deps/impact | MCP JSON payloads | Cutover: thin client delegation |
| 17 | `src/devcouncil/integrations/mcp/handlers/map.py` | Map overview & status | MCP JSON payloads | Cutover: thin client delegation |
| 18 | `src/devcouncil/integrations/mcp/handlers/debug.py` | DAP debug broker | Debug protocol events | Adjunct port in Phase 7 |
| 19 | `src/devcouncil/integrations/mcp/handlers/ast_lsp.py` | AST/LSP queries | LSP JSON payloads | Adjunct port in Phase 7 |
| 20 | `src/devcouncil/verification/gate_selector.py` | Code graph metrics | Gate selection dict | Cutover: thin client status |
| 21 | `src/devcouncil/verification/checks/semantic_diff.py` | Semantic diff analysis | Diff summary dict | Cutover: socket query API |
| 22 | `src/devcouncil/verification/checks/dead_symbols.py` | **PRIVATE INTERNAL REACH**: `devcouncil.indexing.graph.liveness` | Dead symbol list | **WILL BREAK AT CUTOVER** — replace with supported `dead` query API |
| 23 | `src/devcouncil/verification/checks/subsystem_boundary.py` | Subsystem map validation | Boundary error list | Cutover: socket query API |
| 24 | `src/devcouncil/verification/checks/acceptance_corpus.py` | Extraction corpus runner | Test results | Cutover: socket verification |
| 25 | `src/devcouncil/verification/checks/stale_map.py` | **PRIVATE INTERNAL REACH**: `devcouncil.indexing.map_artifacts` | Map freshness bool | **WILL BREAK AT CUTOVER** — replace with daemon freshness banner |
| 26 | `src/devcouncil/verification/checks/corpus_stale.py` | Corpus hash check | Stale status | Cutover: daemon freshness check |
| 27 | `src/devcouncil/verification/checks/liveness_ratchet.py` | **PRIVATE INTERNAL REACH**: `devcouncil.indexing.graph.liveness` | Reachability metrics | **WILL BREAK AT CUTOVER** — replace with supported `liveness` query API |
| 28 | `src/devcouncil/verification/checks/orphan_diff.py` | Graph connectivity check | Orphan list | Cutover: socket query API |
| 29 | `src/devcouncil/verification/checks/wiring.py` | Wiring verification | Violation list | Cutover: socket query API |
| 30 | `src/devcouncil/verification/verify_orchestration.py` | Pipeline verification | Status result | Cutover: socket client delegation |
| 31 | `src/devcouncil/verification/verifier.py` | Core verifier runner | Verification report | Cutover: socket client delegation |
| 32 | `src/devcouncil/verification/wiki_refresh.py` | Wiki generator | Markdown pages | Cutover: socket client query |
| 33 | `src/devcouncil/verification/claims/mapper.py` | Claim graph mapping | Claim mapping dict | Cutover: socket client query |
| 34 | `src/devcouncil/knowledge/wiki.py` | **PRIVATE INTERNAL REACH**: `devcouncil.indexing.graph.communities` | Community clusters | **WILL BREAK AT CUTOVER** — replace with supported community query API |
| 35 | `src/devcouncil/execution/policy_engine.py` | Policy enforcement | Policy decisions | Cutover: socket client query |
| 36 | `src/devcouncil/execution/prompt_builder.py` | **PRIVATE INTERNAL REACH**: `devcouncil.codeintel.store.sqlite` | Direct SQLite queries | **WILL BREAK AT CUTOVER** — replace with supported query API |
| 37 | `src/devcouncil/execution/lease_ops.py` | Task lease graph ops | Lease status dict | Cutover: thin client delegation |

## Private-Internal Reaches Summary (Audit §e)
The 6 files with direct private-internal imports that will break at cutover and MUST be updated to call supported client APIs:
1. `src/devcouncil/cli/commands/ast.py` (`devcouncil.indexing.ast_matcher`)
2. `src/devcouncil/verification/checks/dead_symbols.py` (`devcouncil.indexing.graph.liveness`)
3. `src/devcouncil/verification/checks/stale_map.py` (`devcouncil.indexing.map_artifacts`)
4. `src/devcouncil/verification/checks/liveness_ratchet.py` (`devcouncil.indexing.graph.liveness`)
5. `src/devcouncil/knowledge/wiki.py` (`devcouncil.indexing.graph.communities`)
6. `src/devcouncil/execution/prompt_builder.py` (`devcouncil.codeintel.store.sqlite`)

## Phase 5 (2026-09-10): verification / execution port

Go owns the verify surface; Python packages remain where other CLI surfaces still import them.

### Ported (verified)

| Surface | Location |
|---------|----------|
| Report / Gap / NextAction shapes + golden replay | `backend/go_orchestrator/devcouncil/verify` |
| `devcouncil verify` CLI + `devcouncil_verify_task` / `get_gaps` MCP | `cmd/devcouncil`, `devcouncil/registry.go` |
| Persist `verification_runs` + gap replace | `dc-store` (`run-record`, `gaps-clear`, `gap-upsert`) + Go `store.GapsReplace` |
| Pure diff∩scope / stubs / secrets / coverage / dead JSON | `rust-port/crates/dc-verify` (`lib`, `rigor`, `coverage`, `dead`) |
| Stop-gate decision + skip≠pass | `devcouncil/stopgate` |
| Correction manifests | `devcouncil/correction` |
| Hard-safety gap classification | `devcouncil/gating` |
| `dev verify` CLI shim | `src/devcouncil/cli/commands/verify.py` → execs Go |

### Incremental hard-cut (Phase 7 — closed 2026-09-10)

Python orchestration packages listed below were **deleted**. Decisions and
replacement pointers: [docs/PHASE7_LONG_TAIL.md](../docs/PHASE7_LONG_TAIL.md).

Deleted trees: `verification/`, `execution/`, `planning/`, `gating/`, `executors/`,
`llm/`, `knowledge/`, `reporting/`, `telemetry/`, `campaign/`, `live/`, `ui/`,
`optimization/`, `integrations/`, `skills/`, `codeintel/`, `indexing/`, `storage/`,
and the remainder of `src/devcouncil` except the thin CLI launcher.

Live CLI shims: `dev mcp-server|integrate|skills|verify` → Go `devcouncil`;
`dev map|graph|ast` → Rust `devmap`. All other legacy `dev <name>` commands exit 2
with a recorded retirement message.

### Manvi fold (no reimplementation)

- Provider routing stays in `Manvi/manvi/llm` (adapters under `anthropic/`, `openaicompat/`, `local/`, `gemini/`, `xai/`). Do not port Python `llm/provider.py` / `router.py` into DevCouncil Go.
- Executor profiles map to Manvi `manvi/agents` definitions; coding-CLI spawning stays on the Manvi agent loop, not a second Go executor registry.
- Campaign / live watch / dashboard → Manvi TUI / `manvi watch` / `manvi run` (retired in Python, not ported).

