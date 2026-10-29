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

**Correction (2026-10-29, Fable ruling 1).** The first prototype keyed a payload on file + content + the extractor's version strings and, on a key match, kept the stored payload's derived rows whenever its symbol count equalled the extraction in hand. On v27 the per-generation rows were always written from the extraction in hand; on v28 a same-count change of kind, name, span or literal stayed stale in `payload_nodes`, `payload_literals`, `nodes_fts` and `extraction_json`. That is reachable under the open extract-98 policy (a binary built off a lane writes other output at the same version strings) and by a budget refusal of bytes that parse. `build --full` did **not** clear it: it re-parses every file but reaches `ensure_payload_id` with no affected paths, finds the same key and reuses the payload (`build.rs` sets `affected_paths` empty, `write.rs` `full_rewrite`, every extraction is "fresh", each is `Reused`). Eight fail-before tests (kind, name, span, literal, each differential and `--full`) failed on `480aab5c` and pass now.

*Fix.* The payload identity gains `payload_digest`: 128 bits (two salted SipHash streams) over the three documents the row holds (`parse_outcome_json`, `engine_json`, `extraction_json`), computed on the serializer thread where the documents are made. It is in the unique index (`idx_file_payloads_digest_identity`, replacing `idx_file_payloads_identity`, which a v3/v4 re-run of `CREATE_SCHEMA_V3` on a restamped store would otherwise recreate; the v28 rung drops it again). A different output for one key is therefore a **new payload id**: the retained generations keep reading the rows they were built with, the new generation gets its own, and retention collects the older payload with its rows when no retained range names it. Reuse means the stored row is byte-for-byte this extraction, so the derived rows are the extraction's by identity, not by a count. The count check stays for one purpose only: a reused payload whose rows are missing (a v27 payload no generation named, or damage) is repaired from the extraction in hand, which equals the one it was made from.

Why the whole documents and not a digest of the derived rows: the derived rows are a function of the extraction, so documents that are byte-identical give identical rows, and the digest also covers what only `extraction_json` holds (wiring, routes, imports, calls, `go_package`, which readers in `query.rs` and the cache fallback read). A digest of nodes and literals alone would have let those go stale the same way, and a budget refusal and a clean empty file, both zero symbols, would have collided. One consequence is that **the in-place replacement of a budget refusal by its recovery is gone** (`PayloadState::Replaced` and the "declared interim" `UPDATE` that rewrote what every retained generation read). A refusal is another output, so it is another payload; `a_retained_older_generation_keeps_the_refusal_it_was_built_with`, ignored against card ft-3bdf1450, now passes.

*Costs and side effects.* The v28 rung stamps each v27 payload with the digest of its stored documents (Rust, one payload at a time): on a real v27 store (DevCouncil, 1,893 payloads, 352 MB) the first `build` takes 1.90 s against 0.10 s for a no-op build, three runs at 1.90 s, no empty digest left, integrity ok, and a `build --full` afterwards reused all 1,893 payloads and wrote no row (so the backfilled digest equals the writer's on real data). On a fresh store, `--full` twice in a row also leaves 1,893 payloads and 25,424 symbol rows. A store the first prototype (`480aab5c`) stamped 28 has no `payload_digest`; the open adds the column and stamps the digests from the stored documents (`heal_payload_digest`, the same treatment as an index added within a version), where it had failed with a bare "no such column" (observed with the fixed binary on a store built by the `480aab5c` binary). The bench binary was built at `fec0f386`, before this heal, which costs one `PRAGMA table_info` on a writer open. The cache fallback (`try_get_cached_extraction`) read `generation_files ... LIMIT 1`, arbitrary among the payloads one identity can now name; it serves the newest generation's payload and never a failed parse (the cache does not admit one). Not changed: the `extraction_cache` table is keyed on the same version strings, so a lane binary at an unbumped extract version still reads its predecessor's cached extraction on the incremental path; that is the extract-98 policy question, not a store question, and `--full` bypasses the cache.

**Interleaved bench (after the fix).** Release builds of `dbd0193a` and of `lane/store-v28` at `fec0f386`, `RAYON_NUM_THREADS=4`, `git archive` snapshots, 5 rounds, base and branch alternating order per round, each on a fresh copy walking cold, then three one-file appends, `/usr/bin/time -l`. Load average 11.6 at the start, 12.1 in the middle and 11.8 at the end (other lanes were running), so wall is inflated and noisy; instructions retired are not. `failed to parse` was equal in both arms of every run (1 for DevCouncil, 0 for GitPulse).

| | DevCouncil (1,893 files) | GitPulse (3,071 files) |
|---|---|---|
| per-edit median wall, 15 edits per arm | 2.22 -> 1.41 s (-36.5%) | 3.84 -> 2.75 s (-28.4%) |
| per-edit min wall | 1.54 -> 1.14 s (-26.0%) | 2.80 -> 2.26 s (-19.3%) |
| first edit after cold, median | 1.80 -> 1.30 s (-27.8%) | 2.93 -> 2.41 s (-17.7%) |
| steady-state edits (2nd, 3rd), median | 2.27 -> 1.40, 2.32 -> 1.43 s | 4.00 -> 2.81, 3.93 -> 2.83 s |
| per-edit median instructions | 50.02 -> 40.72 G (-18.6%) | 96.18 -> 81.08 G (-15.7%) |
| per-edit min instructions | 42.63 -> 37.72 G (-11.5%) | 82.48 -> 74.42 G (-9.8%) |
| `persist:write` per edit, median | 1.28 -> 0.39 s | 2.04 -> 0.82 s |
| paired edits where the branch was faster | 15/15 | 15/15 |
| cold wall, min / instructions, min | 6.01 -> 5.84 s / 263.35 -> 263.32 G | 11.00 -> 10.77 s / 511.32 -> 511.00 G |
| cold peak RSS, min | 1146 -> 1152 MB | 2088 -> 2099 MB |
| store after cold, median | 352.4 -> 347.5 MB | 593.2 -> 586.2 MB |
| store after 3 edits, median | 427.2 -> 356.3 MB | 686.3 -> 600.0 MB |

The digest did not cost the per-edit gain: median instructions per edit moved from -19.9% / -16.8% before the fix to -18.6% / -15.7% after (cold instructions are unchanged to 0.01%, the digest being computed on the serializer threads), and the stores are the same size. The wall gate as written (per-edit median improved by 30% or more) is met on DevCouncil (-36.5%) and **not** on GitPulse (-28.4%, against -32.3% in the first run at a load of 12-29); both are noisy at this load, and Fable's re-bench at lower load measured -18% to -26% wall and -6% to -13% instructions, so neither run supports reading the gate as met without the owner replacing it. The first edit after a cold build pays less because the base store holds one generation and the retention prune has nothing to delete yet; the steady state of a long-lived store is edits 2 and onward.

**Read side.** Nine interleaved runs per command on the DevCouncil store, minimum instructions: `search` -0.3%, `search --semantic` -2.5%, `explore` +1.3%, `impact` +1.4%, `dead` +2.9%, `clones` +4.4%, `status` +3.4%, `literals` +2.4%, `literals --exact` **+14.9%** (106.1 -> 122.0 M instructions; it was +12.7% before the fix, so the fix did not change it and it is not a regression the fix caused). Wall is unchanged at the 10 ms the timer reports. The extra is about 12-16 M instructions on every command that opens the store and does not otherwise get cheaper, which reads as a fixed per-open cost of the v28 views, not a cost of the literals query (inferred from the pattern across commands; the open path was not profiled), and it is the largest relative figure only because `--exact` is the cheapest command.

**Identity.** Re-run on the fixed branch (`fec0f386`) against the `dbd0193a` release. Sorted rows of the five compatibility views plus `generation_dead_symbols`, `generation_edges` and the `nodes_fts` join, for the latest and the previous generation, and normalised JSON of `status`, `dead`, `clones`, `search`, `search --semantic`, `explore`, `impact`, `literals` (substring and exact) for fixed symbols, at cold, one-file edit, second edit, revert, delete and full rebuild: three corpora (DevCouncil 1,893 files, GitPulse 3,071, `rust/testdata` 548), 1,782 answers compared (1,494 exact, 288 semantic), **0 exact answers differ**, `PRAGMA integrity_check` ok on all six stores, and `failed to parse` equal between arms (1, 0, 2). One semantic answer differs (DevCouncil, `searchsem.fixture` after the first edit), the same class as before: the semantic ranker sums over `HashMap`s in per-process order, so its answers are compared as the set of items scoring 0.5 or more, and, measured in the first prototype run and not repeated here, base against a second base run differs in 2 of 96. Fields dropped from the comparison, each because it names the arm and not the store: `db_path`, `root`, `pid`, `unix_ms`, `binary_path`, `binary_version`, `schema_version`, `expected_schema_version` and the artifact paths; floats rounded to three places.

**Migration.** A real v27 store (427 MB, written by the base binary) opens under the branch and migrates (the first `build` takes 1.6-2.0 s against 0.12 s for a no-op build afterwards, so the rung costs about 1.5-1.9 s at this size), and the latest and previous generations then read what they read before. A `kill -9` that landed mid-migration (820 KB of uncommitted WAL) left user_version 27, integrity ok, no v28 table, and the base binary still answering from it; the retry completed and read identically. A v27 binary against a v28 store refuses on every query command and on `build` (`schema version 28 can be read only by a reader at schema 28 or newer`), and its `status` reports `reader_ready: false` and names the schema skew.
