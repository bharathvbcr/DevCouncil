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

## Appendix B: v28 prototype results (branch `lane/store-v28`, 2026-10-29)

Status: a branch prototype of §7, not merged, not released. Base `dbd0193a` (schema 27, floor 25); branch schema 28, reader floor **28** (the views keep every v27 column, but the `nodes_fts` rowid no longer carries a generation, so a v27 reader would join it by the old arithmetic and answer from the wrong rows without an error; a floor of 28 makes it refuse instead).

**Shape.** `payload_nodes`, `payload_literals` and the `nodes_fts` rows are written once per payload (rowid `payload_id << 24 | ord_in_file`); `file_rows` and `digest_rows` are validity ranges; `generation_nodes`, `generation_literals`, `generation_file_rows`, `generation_file_digests` and `nodes_fts_map` are views with the v27 column sets. `generation_dead_symbols` and `generation_coverage_gaps` stay per generation. Retention collects a payload with everything derived from it when no retained range names it.

**Interleaved bench.** Release builds, `RAYON_NUM_THREADS=4`, `git archive` snapshots, 5 rounds, base and branch alternating order per round, each on a fresh copy walking cold, then three one-file appends. Load average 12-29 throughout (other lanes were running), so wall figures are inflated and noisy; instructions retired are not.

| | DevCouncil (1,893 files) | GitPulse (3,071 files) |
|---|---|---|
| per-edit median wall, 15 edits per arm | 2.70 -> 1.70 s (-37.0%) | 5.29 -> 3.58 s (-32.3%) |
| per-edit min wall | 1.96 -> 1.36 s (-30.6%) | 3.05 -> 2.83 s (-7.2%) |
| first edit after cold, median | 2.14 -> 1.63 s (-23.8%) | 3.98 -> 3.03 s (-23.9%) |
| steady-state edits (2nd, 3rd), median | 2.82 -> 1.80, 2.86 -> 1.70 s | 5.51 -> 3.65, 5.42 -> 3.95 s |
| per-edit median instructions | 49.98 -> 40.02 G (-19.9%) | 96.53 -> 80.36 G (-16.8%) |
| `persist:write` per edit, median | 1.59 -> 0.46 s | 2.68 -> 1.02 s |
| paired edits where the branch was faster | 15/15 | 15/15 |
| cold wall, min / instructions, min | 7.63 -> 7.20 s / 263.4 -> 262.4 G | 13.57 -> 13.01 s / 511.1 -> 509.7 G |
| cold peak RSS, min | 1145 -> 1160 MB | 2085 -> 2101 MB |
| store after cold, median | 352 -> 348 MB | 593 -> 586 MB |
| store after 3 edits, median | 427 -> 356 MB | 686 -> 600 MB |

The first edit after a cold build pays less because the base store holds one generation and the retention prune has nothing to delete yet; the steady state of a long-lived store is edits 2 and onward. Neither corpus reaches the card's absolute 786 ms. The gate as written (per-edit median improved by 30% or more) is met on both corpora at this load, and the instruction counts, which load does not move, agree in direction at about -17% to -20%. The projection in §7 (27-29%) was low on wall and high on instructions: the prune of the previous generation's copy of every relation (0.57 s on v27) is gone as well as the carry.

**Read side.** Nine interleaved runs per command on the DevCouncil store: `search` -10.1% instructions, `explore` +0.8%, `impact` +1.0%, `dead` +2.3%, `clones` +3.4%, `status` +3.7%, `literals --exact` +12.7% (107 -> 120 M instructions, below the wall-clock resolution), wall unchanged at the 10 ms the timer reports.

**Identity.** Sorted rows of the five compatibility views plus `generation_dead_symbols`, `generation_edges` and the `nodes_fts` join, for the latest and the previous generation, and normalised JSON of `status`, `dead`, `clones`, `search`, `search --semantic`, `explore`, `impact`, `literals` (substring and exact) for fixed symbols, at cold, one-file edit, second edit, revert, delete and full rebuild: three corpora (DevCouncil 1,893 files, GitPulse 3,071, `rust/testdata` 548), 1,782 answers compared (1,494 exact, 288 semantic), 0 exact answers differ. Fields dropped from the comparison, each because it names the arm and not the store: `db_path`, `root`, `pid`, `unix_ms`, `binary_path`, `binary_version`, `schema_version`, `expected_schema_version` and the artifact paths; floats rounded to three places. The semantic ranker is not deterministic between builds of one binary (it sums over `HashMap`s in per-process order), so its answers are compared as the set of items scoring 0.5 or more: base against a second base run differs in 2 of 96 answers, base against branch in 1 of 96 on DevCouncil and 0 of 96 on the other two.

**Migration.** A real v27 store (427 MB, written by the base binary) opens under the branch and migrates (the first `build` takes 1.6-2.0 s against 0.12 s for a no-op build afterwards, so the rung costs about 1.5-1.9 s at this size), and the latest and previous generations then read what they read before. A `kill -9` that landed mid-migration (820 KB of uncommitted WAL) left user_version 27, integrity ok, no v28 table, and the base binary still answering from it; the retry completed and read identically. A v27 binary against a v28 store refuses on every query command and on `build` (`schema version 28 can be read only by a reader at schema 28 or newer`), and its `status` reports `reader_ready: false` and names the schema skew.
