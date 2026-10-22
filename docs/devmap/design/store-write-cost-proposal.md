# DevMap store write cost: measurements and a v28 proposal (ft-f6abefd9)

Status: proposal for the owner. This covers the measurement half only; nothing schema-related is implemented.
Base `f796dd8c`, store schema v27, reader floor 25. Measured 2026-10-22 by lane A, using a scratch, uncommitted build with wall-time accumulators around uncharged regions of `save_generation_timed` (see the appendix).
Load at run start was 15.9–22.4 (median 20.8); n=5, figures are min / median.

## 1. Rows written per one-file edit

Corpus: `rust/`, 1,041 files. Counts are taken after a cold build (generation 1) and after a one-function edit (generation 2).

| Relation | Rows in gen 2 | Differential? |
|---|---|---|
| `generation_literals` | 117,046 | no (not on the card's list) |
| `generation_nodes` | 17,596 | no |
| `nodes_fts_map` | 17,596 | no |
| `nodes_fts` content/docsize | 17,596 | no (re-tokenised) |
| `generation_dead_symbols` | 8,018 | no |
| `generation_file_rows` | 1,041 | no |
| `generation_file_digests` | 997 | no |
| `generation_coverage_gaps` | 60 | no |
| `build_history`, `generations` | 1 + 1 | the `generations` row carries 1.72 MB of `analysis_json` |
| `edge_rows` | 1 opened, 0 closed | yes (range) |
| `unresolved_rows` | 0 | yes (range) |
| `file_payloads` | 1 new | yes |

- **The card's five relations** write 45,248 rows, which matches its 6n+1 shape.
- **Counting every generation-keyed relation**, about 180k rows are written.
- **The largest relation, `generation_literals`, is missing from the card.**

## 2. Where a one-file edit's write time goes

`persist:write` median is 1.235 s (min 1.059).

| Part | Median (s) | Share |
|---|---|---|
| Literals carry (`INSERT … SELECT`, 117k rows) | 0.648 | 52% |
| `unresolved` (already ranged; cost is digest scan + compare) | 0.277 | 22% |
| `edges` | 0.074 | 6% |
| `commit` | 0.066 | 5% |
| `fts` | 0.063 | 5% |
| `nodes` | 0.038 | 3% |
| `dead` | 0.022 | 2% |
| `file_rows` + `digests` | 0.008 | <1% |
| carry decision, `AnalysisSummary` serialise, `generations` insert | ~0.011 | <1% |

The literals carry accounts for essentially all of the reported `other` (0.677 s) on an incremental build.

## 3. Cold-build `persist:write` attribution

Cold-build `persist:write` has a median of 2.96 s (min 2.15). The card's 43–54% unattributed `other` reproduces here: 1.63 s.

| Region | Median (s) | Calls |
|---|---|---|
| Payload JSON serialisation, ×3 per file (137 MB) | 0.924 | 1,041 |
| Literal inserts, uncached `execute` | 0.513 | 117,046 |
| `ensure_payload_id` select + insert | 0.101 | 1,041 |
| `AnalysisSummary` serialise (1.72 MB) | 0.0017 | 1 |
| `generations` insert | 0.0004 | 1 |

- **Attributed:** about 1.54 s of the 1.63 s `other`, leaving roughly 3% of the write unexplained.
- **The audit's guess is refuted.** The `AnalysisSummary` JSON plus the `generations` insert cost about 2 ms.
- **Card acceptance #1** is met by naming these regions as spans: `other` would fall below 15%.

## 4. A 30k-symbol single Swift file

**Shape.** One struct with 30,000 `var vN: Int = 0`, plus a method that reads each one: 967,840 bytes. The card's literal shape (1.30 MB) exceeds the 1 MiB file limit.

**Result.** 30,003 symbols and 90,003 edges in 3.88 s wall (min).
- Extraction takes 3.12 s (81%).
- `persist:write` takes 0.41 s (~10%).
- Symbol insert (`nodes` 0.104 s + `fts` 0.052 s) is about 5.2 µs per symbol, roughly 4% of the build.

**The card's premise does not hold on main.** Extraction dominates, not symbol insert. The card's 1.63× figure was measured on a lane head and was not reproduced (9030d08c was not built).

## 5. Statement reuse (scratch A/B, no schema change)

Routing the fresh-row `generation_nodes` and `generation_literals` INSERTs through `prepare_cached` gave:

| Measure | Before → after |
|---|---|
| Swift 30k, `nodes` span (min) | 0.1006 → 0.0486 s (~2×) |
| Cold-build instructions | 191.49 → 189.91 G (−0.8%) |
| Swift 30k instructions | 41.94 → 41.01 G (−2.2%) |

Load during this A/B was 17–64, so only the `nodes` span result is solid.

## 6. Root cause per relation

- **`generation_literals`, `generation_nodes`, `nodes_fts` + `nodes_fts_map`.** They depend only on one file's content plus extractor identity, which `file_payloads` already keys. Yet they are keyed by generation, and every unchanged file's rows are copied on every build.
- **`generation_file_rows`, `generation_file_digests`.** These record membership per (generation, file). One edit rewrites all 1,041 and 997 rows.
- **`generation_dead_symbols`.** These are whole-graph results. They change for only a few symbols per edit but are rewritten in full.
- **The FTS rowid.** `(generation_id << 32) | ordinal`, with ordinals running across files, so node identity is not stable across generations.
- **`edge_rows` and `unresolved_rows`.** Already ranged (v18).

## 7. Proposed v28

**Payload-scoped tables, written once per payload:**
- `payload_nodes(payload_id, ord_in_file, …)`.
- `payload_literals(payload_id, …)`.
- FTS rows with a stable rowid, e.g. `payload_id << 24 | ord_in_file`.
- `nodes_fts_map` becomes a join of payload rows and file rows.

**Range-scoped tables, opened and closed like `edge_rows`:**
- `generation_file_rows` as `(file_id, payload_id, valid_from, valid_to)`.
- `generation_file_digests`, likewise.
- `generation_dead_symbols`, ranged by `(file_path, symbol_name)`.

**Compatibility:** keep the existing names as views (the v17/v18 playbook): `generation_nodes`, `nodes_fts_map`, `generation_literals`, `generation_file_rows`, `generation_file_digests` and `generation_dead_symbols`.

**Migration (v27 → v28):**
1. Create the payload and range tables.
2. Backfill nodes and literals from the newest retained generation, one row per payload.
3. Convert file rows and digests to ranges.
4. Rebuild `nodes_fts` with the new rowids, using the existing rebuild in `db/maintenance.rs`.
5. Drop the old base tables and create the views.
6. GC payload rows that no retained generation references (`GENERATION_RETENTION` is 2).
7. Prove with `migration_ladder.rs::a_migrated_store_carries_the_same_schema_as_a_fresh_one`.

**Projected effect (inferred from §2, not measured).**
- Literals + nodes + fts + file_rows + digests together cost about 0.76 s of the 1.235 s edit write.
- Removing that brings `persist:write` to about 0.45 s.
- The saving is about 27–29% of per-edit wall.
- The card's two-relation prototype (`generation_nodes` + `nodes_fts_map`) moves about 0.10 s (~4%), so against its own ≥30% gate it is NO-GO.

**Reader floor.**
- If the views stay name- and column-compatible, `MIN_READER_SCHEMA_VERSION` could stay at 25. Prove it by running `tests/reader_floor.rs` against a v28 store with v27 SQL.
- The `CROSS JOIN nodes_fts_map m` planner hint (`db/query.rs:281`) changes once the map is a view, so re-benchmark FTS queries.
- Node `ordinal` becomes non-contiguous across files; check every reader that relies on extraction order.

## 8. Readers affected

- **DevCouncil:**
  - `devmap-store` (query, generation, maintenance, expected-columns, schema).
  - `devmap-query` (semantic), `devmap-analyze` (liveness), `devmap-serve` (daemon), `devmap-cli` (status, doctor, repair).
  - `rust/tools/fanout.sql`, `benchmarks/map_bench.py`.
  - The store/CLI/analyze tests, including `test_findings_suite.rs::b3_one_file_edit_write_set_…` and `a_one_file_edit_writes_one_file_of_rows.rs`.
- **GitPulse:**
  - It vendors devmap-store, devmap-analyze, devmap-query, devmap-resolve and devmap-extract.
  - `SUPPORTED_STORE_SCHEMA = CURRENT_SCHEMA_VERSION` (`codeintel/mod.rs:34`), so it moves in lockstep with any bump.
  - Its fixtures INSERT directly into `generation_file_rows`, `generation_nodes` and `nodes_fts_map` (`codeintel/mod.rs:3001-3128`, `mcp/devmap_parity.rs:572-587`) and break once those become views.
  - Re-vendor with `scripts/vendor-crates.mjs`, then run `check:vendor-schema` and `check:schema-parity`.
- **Manvi and Jarvis:** they pin DevCouncil, so a pin bump follows the report-and-ask rule.

## 9. Recommendation

1. **Non-schema first:**
   - `prepare_cached` for the two fresh-row INSERTs;
   - one statement for the per-path literal carry;
   - named spans for payload serialisation, literal insert and `ensure_payload_id` (which meets card acceptance #1).
2. **If you want a schema change,** do not prototype the card's two-relation pair (about 4%). Take payload-scoped literals + nodes + FTS together with file-row and digest ranges as one unit (projected 27–29%). That sits borderline against the ≥30% gate, so it is a judgement call.
3. **The GO decision is the owner's.** It requires the lockstep GitPulse re-vendor and a consumer rebuild.

## Caveats

- Load never fell below 15.9, so wall figures are floors.
- The corpus is `rust/` only.
- The Swift shape differs from the one the card used.
- The instrumented build adds a small, unquantified timer overhead.
- §7's projection is inferred, not measured.

## Appendix: scratch instrumentation spans

All spans were added in `rust/devmap-store/src/db/write.rs` (not committed). Each marks one region:
- duplicate-path scan;
- transaction begin and root bind;
- `AnalysisSummary` serialise, with byte count;
- `generations` insert;
- carry decision and file-rows carry;
- per-file payload JSON serialise, ×3;
- `ensure_payload_id`;
- fresh literal insert, per row;
- literal carry `INSERT … SELECT`;
- `scrub_secret_values`;
- total.

The cache variant wraps `tx.prepare_cached(sql)?.execute(params)` around the two fresh-row INSERTs.
