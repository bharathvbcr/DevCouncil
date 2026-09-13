# DevMap store footprint and retention audit

Status: **verified locally on 2026-09-12**. The 138.8 MiB cold store is mostly live extraction and attribution data. It contains no free pages, extra retained generations, extraction-cache rows, retry rows, or nonempty WAL. The audit found and fixed a separate retention defect: unique file renames left interned path rows behind indefinitely. That defect does not explain the original cold footprint.

## Measured evidence

The preserved competition stores were opened read-only. Reclaim experiments used SQLite backup copies under `.devcouncil/audit/store/`; source SHA-256 values before and after matched. `storage-evidence.json` records full schema, per-table counts, every index, page accounting, logical and filesystem-allocated sizes, sidecars, integrity checks, source hashes, and per-table multiset digests for each experiment. `inspect_storage.py` records the procedure and refuses to overwrite existing experiment copies.

These are the preserved benchmark stores made by the earlier dirty binary, not clean-build reproductions. The clean release baseline is separately identified in `.devcouncil/audit/baseline/provenance.json`; its SHA-256 is `cce843d25f48323c90bf92079ee2b7be79af5b1bd5a63c4661acc5aa53c8bddd`, commit `ee07c183b127e7291c38f993f4d71ca75f1121dc`, with an empty tracked/untracked status before compilation. See [the update profile](UPDATE_PROFILE_AUDIT.md) for that run.

| Measurement | Preserved cold run 1 | Preserved run 3 after edit/restore |
|---|---:|---:|
| Main database | 145,588,224 B / 138.844 MiB | 158,695,424 B / 151.344 MiB |
| Page size | 16,384 B | 16,384 B |
| Pages | 8,886 | 9,686 |
| Free pages | 0 | 411 / 6.422 MiB |
| Retained generations | 1 | 2 |
| File membership rows | 857 | 1,714 |
| Distinct extraction payload rows | 857 | 858 |
| Nodes / FTS documents | 10,193 | 20,387 |
| Physical edge rows | 33,789 | 33,790 |
| Physical attribution ledger rows | 162,590 | 162,590 |
| Extraction cache / retry rows | 0 / 0 | 0 / 0 |
| Build history rows | 1 | 5 |
| Interned paths / unreachable IDs | 888 / 0 | 888 / 0 |
| Nonempty WAL | None | None |

The retained membership rows share payloads; a second generation does not duplicate the 74 MiB extraction table. Edge and attribution validity ranges also share unchanged rows. Nodes, FTS documents, analysis JSON, dead-symbol rows, and per-generation digests are copied per retained generation. The source retention constants are two generations and 500 build-history entries in [schema.rs](../../rust/devmap-store/src/schema.rs).

## Where the cold bytes live

`dbstat` measures allocated B-tree pages, payload, and unused space inside allocated pages. It excludes free pages and pointer-map pages, which the raw accounting reports separately. Unused space inside a populated page is not equivalent to an entirely reclaimable page. [SQLite DBSTAT documentation](https://sqlite.org/dbstat.html).

| Cold-store component | Allocated MiB |
|---|---:|
| Extraction payloads | 74.109 |
| Attribution ledger | 45.391 |
| Edge rows | 5.250 |
| Attribution classification index | 4.047 |
| Attribution callee index | 2.703 |
| Generation nodes | 1.563 |
| FTS document content | 1.484 |
| Generation analysis JSON | 0.938 |
| FTS posting data | 0.875 |
| Dead-symbol rows | 0.656 |

All ordinary indexes together occupy **7.938 MiB**. Total unused bytes within B-tree pages are **3.814 MiB**. The two largest tables account for **119.5 MiB, approximately 86%** of the entire cold store.

Parsing each stored extraction JSON, the serialized values for references account for 37.882 MiB, calls 18.746 MiB, local bindings 7.157 MiB, symbols 3.303 MiB, scope locals 3.172 MiB, and imports 0.681 MiB. These figures exclude JSON keys/separators and row metadata; they are a breakdown of content rather than a second database-size accounting.

The payload serves incremental re-resolution: [try_get_cached_extraction](../../rust/devmap-store/src/db.rs) reads retained payloads through the full content/language/grammar/analyzer identity, and [the resolver](../../rust/devmap-resolve/src/resolver.rs) consumes scope locals and name/type references. [Extraction::for_durable_store](../../rust/devmap-extract/src/model.rs) already removes source text, diagnostics, and redundant call references while preserving receiver-binding information. Deleting these fields without proving equivalent re-resolution would change the product contract.

The physical attribution ledger has 89,421 `uninferred_receiver`, 49,600 `external`, 18,351 `builtin`, 2,087 `local_binding`, 1,683 `module_path`, 1,069 `no_namesake`, 276 `unresolved`, and 103 `host_global` rows. The reported **91,784 remaining attribution sites** are a narrower metric than the physical ledger total. Neither a larger ledger nor its removal establishes higher accuracy. Confidence, coverage gaps, and incomplete-traversal warnings remain required.

**Inference:** the major difference is retained information and representation, not an accumulation of stale cold data. This establishes why the current format is large; it does not establish that its representation is irreducible. Normalizing repeated strings or restructuring extraction serialization would require a separately measured change with cold/incremental/query equivalence and migration tests.

## Comparison with the inspected competitor artifacts

| Preserved artifact | Measured data | Dominant components |
|---|---|---|
| Graphify archive 1 | 44,552,449 B across 723 files, approximately 42.5 MiB | Graph JSON 16,895,678 B; extraction/cache files 27,431,065 B; manifest and marker 225,706 B |
| CodeGraph archive 1 | Main SQLite database 66,154,496 B / 63.090 MiB | Unresolved references 10.770 MiB; edges 7.941 MiB; ordinary indexes 35.641 MiB total |
| CBM archive 1 | Main SQLite database 70,385,664 B / 67.125 MiB | Nodes 12.375 MiB; edges 9.313 MiB; LSP surface 7.875 MiB; node vectors 6.250 MiB; token vectors 4.875 MiB; ordinary indexes 23.438 MiB total |

The archive file totals can differ slightly from the original timing-stage directory totals because they were inspected later; the earlier report rounded both Graphify totals to 42.5 MiB. The CBM main database excludes its separately inventoried config database and log files. CodeGraph holds 14,120 nodes, 52,054 edges, 55,736 unresolved references, and 727 files. The later CBM archive inspection holds 18,655 nodes and 98,891 edges, which differs from its earlier response-stage inventory; no equality of snapshot/counting scope is assumed.

CodeGraph and CBM have no free pages in these snapshots. Their page sizes are 4 KiB and 64 KiB, respectively. The indexed content, definitions of nodes/edges, auxiliary data, file scope, and provider availability differ. Dividing bytes by unlike node totals would not measure equivalent compression or accuracy. No competitor files were changed, vacuumed, or removed.

## Reclaim behavior on independent copies

All table multisets, including FTS shadow tables, matched before and after each reclaim. Each copy passed integrity and foreign-key checks. Times below are single diagnostic trials using Python SQLite 3.53.4 on this host; they are not product latency medians.

| Copy and operation | Before → after | Reclaimed | Elapsed |
|---|---:|---:|---:|
| Cold, incremental vacuum | 138.844 → 138.844 MiB | 0 | 0.05 ms |
| Cold, full vacuum | 138.844 → 137.719 MiB | 1.125 MiB / 0.81% | 230.77 ms |
| Post-edit, incremental vacuum | 151.344 → 144.922 MiB | 6.422 MiB | 8.57 ms |
| Post-edit, full vacuum | 151.344 → 143.359 MiB | 7.984 MiB | 244.05 ms |

The normal store uses incremental auto-vacuum, a strict greater-than-5% free-page threshold, a 256 MiB per-pass byte cap, and WAL checkpoint outcomes that distinguish completed truncation from a reader holding frames. The post-edit copy has 4.24% free pages, so normal policy correctly retains those pages for reuse; the diagnostic forced reclaim deliberately bypassed that threshold. A full rewrite would recover little from the cold store and does not explain the original storage gap. [SQLite incremental-vacuum and checkpoint semantics](https://sqlite.org/pragma.html).

## Demonstrated defect and fix

**Verified defect:** 64 generations that each rename one source file retained all 64 historical interned paths, although ordinary generation retention kept only the last two payload sets. No code retired `paths` rows. [PathRanks::read](../../rust/devmap-store/src/db.rs) reads and sorts that entire table for edge queries, so indefinite rename churn also increases read work.

The fix extends the existing [generation prune transaction](../../rust/devmap-store/src/db.rs): remove a path ID only when no node, extraction payload, file membership, file digest, edge source, or edge target still references it. Retained generations and edge-only endpoints keep their IDs. The transaction protects the full prune from partial retirement on error. No schema, retention-window, confidence, access, or dependency change is involved.

A seven-trial diagnostic of the added anti-join on the post-edit backup measured 4.36 ms median, with a 13.89 ms first trial and 4.02–4.47 ms thereafter. It removed zero IDs, as expected from the snapshot inventory. This is added bounded maintenance work; no edit-latency improvement is claimed. The change adds 18 production lines and removes none.

[retention_churn.rs](../../rust/devmap-store/tests/retention_churn.rs) adds six tests:

- 64 unique renames plateau at the last two paths.
- Deleting every file retains the old path until its last generation leaves the retention window.
- A trigger-injected path-deletion failure rolls back generations, nodes, payloads, FTS documents, and paths; retry succeeds after removing the injected failure.
- An existing SQLite read snapshot preserves its old paths while a writer prunes; the next snapshot sees the new paths.
- Edge-only source and target paths survive until their last retained edge range ends.
- A persistent `Store::search_fts` reader observes exact new names/paths and absence of the removed symbol across 32 rename/prune cycles; each committed store passes a fresh integrity check.

The first five failed against unmodified production code; the raw pre-fix log is retained. During test construction, a node-count expectation was corrected to use the observed nonempty pre-prune counts, and the search tuple assertion was corrected to its documented column order. These test-development failures are preserved separately from the defect regression evidence.

## Verification and remaining limits

`cargo test --locked -p devmap-store` completed with **235 passed, zero failed, one ignored**, across 19 test targets. This includes the six new tests, existing transactional/crash fault injection, WAL/readers, generation validity ranges, cache retention, migration checks, and 256 concurrent queue-edit sessions. Formatting and diff checks passed. Full workspace verification belongs to the parent audit; this report does not substitute the store suite for those gates.

A diagnostic `PRAGMA integrity_check` through a long-lived inspector on bundled SQLite 3.53.2 reported an FTS segment error after another connection optimized the index. Fresh connections using the same bundled version and Python SQLite 3.53.4 passed, and the persistent production search test passed. This supports a stale diagnostic-connection state explanation, not a demonstrated on-disk corruption. The tests verify committed disk integrity through fresh connections and independently verify persistent search. The underlying SQLite diagnostic behavior is not claimed fixed.

The single ignored existing test is not counted as passed. These tests bound specific local workloads; they do not establish power-loss durability on physical hardware, indefinite resource bounds under stalled readers, performance on other filesystems/platforms, universal language coverage, or complete confidence in every store behavior. The earlier dirty binary's missing source patch remains a provenance gap. This audit supplies measured footprint attribution and a proven retention fix, while preserving those limits.

## Qualified storm, kill, and restart run

**Verified:** the corrected [daemon storm test](../../rust/devmap-cli/tests/daemon_storm_soak.rs) passed all **12 cycles** against clean source commit `962ea5f07921ea925e3b70821c5487472c8b57ea`. The fixture SHA-256 was `d4ab3a7404d22736a2311ac18addc917ab0a8144c5a2aa91456246688496126b`; the tested release SHA-256 was `d6628af04241e3e12853ce279ed1ba60481364e8ca8140ec8cd12ec59cba65db`, matching `.devcouncil/audit/delivery-release/devmap`. Source commit, tree, clean status, fixture, Cargo.lock, and binary hashes matched before and after the run. No other compiler or performance work ran concurrently in this audit.

The exact invocation, from `.devcouncil/audit/candidate-source/rust`, was `DEVMAP_SOAK_CYCLES=12 cargo test --release --locked --offline -p devmap-cli --test daemon_storm_soak a_storm_kill_restart_cycle_converges_on_the_cold_build -- --ignored --exact --nocapture`, with no `CARGO_TARGET_DIR` override. The recorder imposed a 900-second outer timeout and retained stdout, stderr, invocation, source identities, runner snapshots, measurement, and qualification result in `.devcouncil/audit/storm12-qualified/`. It reported exit 0, no timeout or output cap, **157.125 seconds** elapsed, and **156.62 seconds** for the native test: one passed, zero failed, four fixture regressions filtered out. Those four regressions were run separately as described below.

The fixture starts with 200 Python seed files and rotates through four shapes three times: 10,000 creations, renaming the whole churn directory, deletion/recreation with 500 different files, and 1,000 `a → b → c → a` rename loops. Each cycle requests SIGKILL for a daemon and starts its successor without manually cleaning the endpoint. Later creation/rename rounds retain the preceding 500 recreated files and `cycle_a.py`, so their churn directory contains 10,501 files.

| Cycle | Shape | Quiescent nodes | Quiescent edges | Successor RSS, KiB |
|---|---|---:|---:|---:|
| 0 | Create | 20,600 | 10,600 | 164,912 |
| 1 | Rename directory contents | 20,600 | 10,600 | 185,248 |
| 2 | Delete/recreate | 1,600 | 1,100 | 79,904 |
| 3 | Rename loops | 1,602 | 1,101 | 50,960 |
| 4 | Create | 21,602 | 11,101 | 179,248 |
| 5 | Rename directory contents | 21,602 | 11,101 | 186,080 |
| 6 | Delete/recreate | 1,600 | 1,100 | 87,328 |
| 7 | Rename loops | 1,602 | 1,101 | 21,472 |
| 8 | Create | 21,602 | 11,101 | 152,544 |
| 9 | Rename directory contents | 21,602 | 11,101 | 183,968 |
| 10 | Delete/recreate | 1,600 | 1,100 | 86,608 |
| 11 | Rename loops | 1,602 | 1,101 | 60,928 |

Each cycle produced two consecutive empty-queue observations. After the last cycle, the test passed its cold-build comparisons of node/edge counts and complete symbol membership by file, name, and kind, and its final socket/endpoint-lock absence checks. The last queue snapshot is the 1,602-node, 1,101-edge row above; the test does not separately print the final cold counts. All 12 RSS readings were positive. Discarding the first three readings, the remaining four/five-sample half-means were **125,904 → 101,104 KiB, −19.7%**, below the test's 25% growth limit. The wrapper independently checked the cycle sequence, shapes, positive counts/RSS, test receipt, and recomputed RSS result.

The fixture itself required correction before this result could qualify: the previous helper discarded filesystem errors, and its mass rename mutated a live directory iterator that could revisit renamed entries. The canonical helper now propagates create, enumeration, write, rename, and deletion errors with cycle/path context and completes directory enumeration before renaming. Three deterministic blocked-filesystem regressions and one eight-shape first/later-round file-membership regression all failed against the preserved old helper; all four passed after the correction. Their logs and the old helper are retained under `.devcouncil/audit/storm-fixture/`; the old helper SHA-256 is `6254cf5704adf9d67339d3d9caac591cad96be02a99ee2b395ee4363d95b9d65`. The earlier 12-cycle run under `.devcouncil/audit/storm12/` remains unchanged and hash-checked; its weaker fixture controls are not substituted for the corrected run.

**Remaining storm-harness limits:** queue emptiness is checked per cycle; cold equivalence is checked only after the final cycle. Exact edge tuples, confidence, and resolution are not compared. RSS samples come from different restarted processes handling different shapes, so their declining half-means do not prove a continuous daemon's memory plateau. `Daemon::terminate` can fall back to SIGKILL after 30 seconds without failing the test, and `kill9` discards kill/wait errors. This run therefore does not establish graceful shutdown at every cycle or independently verify successful delivery of every requested signal; its outer timeout bounds the whole invocation. The separate strict soak and real-process shutdown suite distinguish graceful from forced shutdown. This synthetic Python workload on one macOS host does not establish universal language, filesystem, crash, or resource coverage.
