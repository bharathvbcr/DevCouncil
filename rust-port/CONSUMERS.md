# Consumer Contracts Ledger (Task 1.4)

This document tracks all 37 external consumer files in `src/devcouncil/` that interface with code intelligence (`devcouncil.indexing` / `devcouncil.codeintel`), their touched surfaces, expected payload shapes, and tri-state expectations (X14: unknown ≠ verified-zero).

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
