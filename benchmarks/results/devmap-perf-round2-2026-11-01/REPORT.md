# DevMap performance, round 2 (2026-11-01)

Branch `claude/devmap-perf-main` (the timed binary is `ec99c364`; the soak harness and this report were committed after it), on top of `main` 7926697d. Nothing is pushed or merged to `main`. No schema constant changed: every binary reports store schema 29, code graph schema 2, extract schema 98. No dependency was added.

**Headline.** A steady-state one-file rebuild of the GitPulse corpus takes 2.88 s (median) against 3.94 s on `main` and 6.70 s on the old dbd0193a build. The merged binary was faster than both `main` arms in 5 of 5 interleaved reps, and its slowest rep (3.00 s) is below `main`'s fastest (3.44 s). Most of the gain is the prune (0.45 s → 0.03 s) and resolve (0.94 s → 0.66 s). The cold build total is unchanged within noise, and cold write is slightly faster. The output is byte-identical to `main`.

The spec's targets (§6.6) are not met: one-file p95 ≤ 120 ms and peak RSS < 512 MB are both missed. Neither can be reached without a schema change and a larger redesign; see "Spec targets".

## Method

- **Corpus.** Frozen GitPulse at d976b431: 3,072 files, 52,425 symbols, 194,039 edges, 605,844 unresolved rows. Every arm got a private copy.
- **Harness.** `benchmarks/devmap_build_ab.sh` (committed, 4362e980) runs 5 interleaved reps with the arm order rotated each rep.
- **Arms.** old = dbd0193a, main = 7926697d, merged = ec99c364, plus main2 = the `main` binary again as a same-binary noise floor.
- **Build kinds.** Each rep runs cold (`--full`), inc1 (one file changed) and inc2 (another file changed, which prunes back to 2 generations: the steady state).
- **Timings.** All times are the in-process `total` and per-stage spans from `build --json`, not parent wall time.
- **Host load.** Load average was 33-38 during the run (other agent sessions were active), so read every gap against the main/main2 spread.
- **Data.** [`runs.tsv`](runs.tsv) and [`summary.txt`](summary.txt) in this folder.

![build time by stage](build-stages.svg)

![steady-state one-file rebuild, every rep](inc2-reps.svg)

## Baseline vs after (min / median of 5, seconds)

| Kind | Stage | old dbd0193a | main | main2 (noise) | merged | merged beats both main arms |
|---|---|---|---|---|---|---|
| cold | total | 10.19 / 11.66 | 11.53 / 12.73 | 10.40 / 11.59 | 10.79 / 12.02 | 2/5 (noise) |
| cold | write | 3.98 / 4.18 | 4.15 / 4.47 | 3.67 / 4.07 | 3.56 / 3.99 | 4/5 |
| cold | resolve | 0.88 / 1.02 | 1.02 / 1.06 | 0.84 / 1.08 | 0.57 / 0.83 | 5/5 |
| inc1 | total | 3.27 / 4.33 | 3.19 / 3.46 | 2.95 / 3.56 | 2.53 / 2.93 | 3/5 (suggestive) |
| inc1 | resolve | 0.66 / 0.87 | 0.90 / 1.03 | 0.97 / 1.12 | 0.60 / 0.63 | 4/5 |
| inc2 | total | 5.78 / 6.70 | 3.44 / 3.94 | 3.45 / 4.08 | **2.81 / 2.88** | **5/5** |
| inc2 | prune_generations | 1.26 / 1.56 | 0.45 / 0.55 | 0.43 / 0.52 | **0.03 / 0.03** | 5/5 |
| inc2 | resolve | 0.90 / 1.11 | 0.94 / 1.19 | 1.04 / 1.21 | 0.66 / 0.69 | 5/5 |
| inc2 | analyze | 0.75 / 0.87 | 0.53 / 0.62 | 0.55 / 0.59 | 0.45 / 0.49 | 5/5 (suggestive, see M1) |

For reference, the noise arm (main) beat main2 in 0-4 of 5 reps per cell.

**Write share** (write / total, median): old 39.5% cold, 47.5% inc1, 35.0% inc2. main 35.1 / 23.8 / 19.0%. merged 33.2 / 25.1 / 22.0%. The merged binary's incremental write share rises only because every other stage shrank. Its absolute write time is equal or lower: inc2 write 0.63 s against main's 0.75 s. Persist (write + prune + vacuum) at inc2 is 24.0% on merged against 32.5% on main.

**DB bytes** are the same for main and merged: 272.6 / 279.6 / 286.6 MB after cold / inc1 / inc2. Old reaches 593-687 MB. This round did not change the store's size. The size cuts need schema v30 (see Decisions).

**Query latency** (`query-latency.txt`): one cold store per arm, 10 interleaved reps, load average 8-27. The branch does not touch the read path, and none of the three queries moved:

| Query | main | main2 (noise) | merged |
|---|---|---|---|
| `search` | 19 ms | 18 ms | 19 ms |
| `impact` | 132 ms | 129 ms | 129 ms |
| `explore` | 351 ms | 349 ms | 345 ms |

**Parse coverage** is identical between main and merged. Both cover 52,425 symbols, with 85 partial-parse files, 51 pattern-recovered files and 1 charged partial. The old build had 52,424 symbols and 192,999 edges. Under CPU starvation the merged binary now recovers every budget-refused file; see the budget lane.

## Correctness

- **Graph parity** (`benchmarks/devmap_graph_parity.sh`): merged vs main, all three hashes identical (nodes `89681f14…`, edges `d5f187c2…`, unresolved `1662e6f2…`).
- **Ledger parity after one incremental build** (`benchmarks/devmap_ledger_parity.sh`): `cmp`-identical across 140,380 texts and 605,844 rows, live and closed.
- **Lane verifiers** (Opus) also checked:
  - insertion order in 22 tables;
  - `analysis_json` and `devmap dead --json`;
  - `foreign_key_check` and `integrity_check` after a real prune.

## What landed (merged into the branch, each Opus-verified)

| Lane | Commits | Change | Evidence |
|---|---|---|---|
| A/B harness | 4362e980 | `benchmarks/devmap_build_ab.sh` | used for every number here |
| write | e82ca803 (482c01a2), 922a2402 | Single-row ledger insert replaces the 64-row chunk; streamed `unresolved_texts` pool | Undoes a `main` regression (ce71cae2). Cold `unresolved` span 23-38% faster in 10/10 pairs. Incremental span about 17-20% faster at the median, 9/10. New `devmap_ledger_parity.sh`. |
| prune | 213772a4 (55e46eba) | Prune with foreign keys off plus one scoped `foreign_key_check` per parent; ANDed anti-joins; texts anti-join skipped when no ledger row was deleted | Root cause of the 835 s Vani prune: 3 unindexed FK child columns, so each retired row scanned 606k ledger rows. Steady-state prune 0.69 s → 0.04 s. Churn (`git mv` of 1,337 files): base never finished inside 420/900 s, fixed 1.7-3.5 s. 4 tests fail pre-fix. |
| budget | 2a99764b (2a8a11b9), 4efe6687 | A 240 s retry-phase deadline replaces the 16-file retry cap; `parse_budget` JSON key plus a stderr diagnostic; `build_ms` = the time the writer lock was held | Under 3 starved concurrent builds, base dropped 32-38 files in a generation reported ok (a correctness bug); fixed recovered every file in 9/9. Cost: +18-22 s in that starved case only. |
| targets (soak) | c49e64fb (0a3f14d7), 5f676ee9 (fmt, doc citations, logs) | soak measures the §6.6 targets: incremental p95, peak RSS, `--readers N` | see "Spec targets" |
| mapping | ec99c364 (29adec4e) | M3: `par_sort_by` (stable) for resolve edges and ledger. M1: `dead_clusters` reuses liveness's sets (a refactor) | Resolve 1.4-1.8× faster in this A/B. Mutation-tested tie-order pins. devmap-analyze 765/0, devmap-resolve 2123/0. |

Diff against `main` at ec99c364, before the soak merge: 35 files, +2,107 / −255.

**Tests.**
- **Tests that fail on the pre-fix code:** prune (4), budget (`build_ms_is_the_locked_build`, `a_build_names_its_parse_budget_outcome`, plus the `budget_retry` updates), and mapping's feature-off fix.
- **Perf-only changes** (the ledger insert, the parallel sort, the M1 refactor) cannot have a test that fails before the fix. They are pinned by parity and by mutation checks instead.
- **Load flakes:**
  - `build_ms_is_the_locked_build` lock-wait test, 2/7 under load. It fails loudly and never falsely passes.
  - `head_stamp.rs` 2 s git timeouts and the `attack_v29_zstd` straddle; both pass when rerun serially.
  - devmap-resolve's `attack_ccf_lsp` spawn deadline, which passes in isolation.

## Spec targets (docs/architecture/UNIFIED_RESEARCH_AND_ARCHITECTURE_REPORT.md §6.6)

The figures in this section come from the targets lane's soak harness, merged as c49e64fb. It extends `rust/tools/soak` with per-build timing, nearest-rank p95, peak RSS and a `--readers N` mode. Its data and logs are in `benchmarks/results/devmap-targets-soak-2026-11-01/`. Its candidate binary is 213772a4 plus soak, which has the write and prune lanes but not budget or mapping. The Opus verifier reproduced every verdict on its own binary built from that branch's head. Its times are **parent-side wall time**, so they are not comparable with the in-process `total` above.

| Target | Bar | Measured | Verdict |
|---|---|---|---|
| One-file incremental p95 | ≤ 120 ms | Lane: pooled N=50 per run, best min 2.3 s, best p95 6.3 s; same-binary medians spanned 2.9× (2.9 s vs 8.4 s) under load 11-20. Verifier: N=40, min 2.75 s, median 3.50 s, p95 5.37 s at load 40. | **Not met** (about 20× at the minimum) |
| Peak RSS, incremental | < 512 MB | 1.74 GiB on the soak branch's head (write and prune lanes; lane and verifier) vs 1.78 GiB on `main` (lane only; the verifier did not rerun it); cold about 2.06 GiB | **Not met** (3.4×) |
| 1 writer + 8 readers | 0 SQLITE_BUSY | Lane: 610 s, 84 builds, 3,299 queries; query median 1.28 s, p95 3.66 s. Verifier: 332 s, 98 builds, 7,199 queries. Both runs: 0 busy, 0 errors, 0 timeouts. | **Met as measured, 2 runs** |

**Soak harness checks.**
- **Tests.** devmap-soak has 69 lib tests and 9 shutdown tests; the verifier mutation-killed 6 of 6.
- **Clippy** is clean.
- **Rustfmt.** The verifier's one blocking item was that the new soak code failed `cargo fmt`. I fixed it after the merge, along with three doc citations it flagged, and committed the soak logs as `.log.txt`, because `.log` is gitignored.
- **What the readers gate does not prove.** A reader counts as "overlapped a build" at the process level, not inside the persist write transaction. An exit-0 answer that says `Unavailable` would count as ok; whether any occurred is unverified.

**Caveats on these verdicts:**
- **The wall-clock gap.** The in-process `total` does not count a gap of min 0.37 / median 1.63 / p95 2.71 s (N=20, under load). Samples point at `dc_proc::git_version` spawns. That gap is a finding of its own; its cause is unverified.
- **The readers result.** The store's 5 s `busy_timeout` turns a transient BUSY into latency. So "0 BUSY" cannot be told apart from "BUSY absorbed into a 3.66 s p95". A stronger gate is a third run plus a probe arm with `busy_timeout=0`.
- **The corpus.** The GitPulse corpus is not the spec's Medium size. It has about 1.3M call/reference sites against the spec's 150k, so 512 MB is about 8× stricter here. The RSS investigator estimates 0.4-0.6 GB on a corpus matched for call volume (inferred, not measured).

**Why 120 ms is not reachable without a schema change.** The targets lane's prioritizer, checked against the code:
- A one-file build today re-materializes all 3,071 extractions (558 MB decoded), rebuilds the resolver index, re-resolves and re-analyzes the whole tree, and re-hashes every row.
- Even a design proportional to the change still runs analysis over all edges. Just reading the 194k stored edges costs about 0.13-0.2 s.
- The no-op floor is a 130 ms scan plus about 40 ms for git plus process start.

Estimated milestones on a quiet host (inferred):

| Step | One-file rebuild |
|---|---|
| Today | ~2.5 s |
| After local cuts C1-C4 | ~1.2-1.5 s |
| After C5 phase one (stub-load, resolve only the affected closure, splice stored rows) | ~0.3-0.5 s |
| After C5 plus T8 (per-file analysis contributions, a schema bump) | 120 ms |

C1 (the parallel sort) is already landed as M3.

## Left undone, and why

- **C2 step 1** (overlap the git HEAD lookup with extraction; at least 40 ms) and **C3(a)** (allocation-free `is_vendored_path`; inferred 0.15-0.25 s). Both are schema-free and low-risk. They were not run here because a gain that small cannot be resolved on this host at load 33-38: every cargo call waited 15-30 min on the slot gate, and same-binary medians spanned 2.9×. Proposed as a follow-up.
- **Deferred mapping items:**
  - M2: run the analyze passes in parallel; needs a determinism audit.
  - M5: memoize the vendored-path check in `resolution_rate.rs`, which is on the avoid list.
  - X3: `site_at` in the literal index is quadratic; 3.6% of extract CPU.
  - X4: tree re-walks; 10-20% of extract CPU, needs its own lane.
  - X2: deadline poll cadence; 4%.
  - X1: largest-first scheduling; ≤ 0.15 s modelled.
  - M6: resolver:index is unmeasured.
- **Budget items:**
  - O2-c: a CPU-time parse budget.
  - O2-e: daemon drains keep single-file refusals.
  - F3: a CPU/priority band in the timings.
  - No daemon-path test for `build_ms`.
- **Storage lane S3** (rebuild 3 indexes after cold builds) is verified but **not merged**: −6.5 MB (−2.4%) for +about 1 s per cold build, against the write-speed goal. The verifier refuted the lane's "within noise" claim.
- **Coverage gap:** no unit test checks that the `lib.rs` call site hands `dead_clusters` the liveness sets rather than empty ones. Only corpus parity guards that.

## Decisions for you

| # | Decision | Exact delta | Cost | Recommendation |
|---|---|---|---|---|
| 1 | Schema v30 bundle: L1 (template the reason text, −42.3 MB), L2 (intern symbols for the ledger and edges, about −50 MB; about −93 MB / −33% together with L1), L3 (drop the second `dead_symbols` copy, −7.6 MB) | `CURRENT_SCHEMA_VERSION` 29→30, `MIN_READER_SCHEMA_VERSION` 29→30 | Migration 29→30; older readers can't open new stores; GitPulse re-vendor, Manvi and Jarvis pins, every installed consumer re-verified | Yes, and bundle T8 into it so the re-vendor happens once |
| 2 | T8: store per-file analysis contributions, the only route to 120 ms | Same constants (fold into v30) | New relation written in the same transaction; analysis rewritten to combine contributions | Yes if 120 ms matters; design it with C5 |
| 3 | C5 scope: a proportional incremental build plus one shared CLI/daemon pipeline | None for phase one | Large cross-crate refactor touching devmap-cli/resolve/store/serve | Approve as its own lane after this branch lands |
| 4 | Svelte repair (88dc6cbd) shipped without an extraction bump | `EXTRACTION_SCHEMA_VERSION` 98→99 | Every store re-extracts Svelte on the next build | Yes, otherwise cached v98 Svelte payloads stay stale until `--full` |
| 5 | Merge the storage lane S3 | none | +about 1 s per cold build for −2.4% size | No |
| 6 | Merge this branch to `main` | none | The `devmap-analyze/src/lib.rs` hunk (7 lines) will conflict with the swarm's uncommitted edits | Yes, after you review |
| 7 | History: WIP commit 60a0b496; b68aa328's subject still says `perf` (relabelled by the empty commit 29adec4e) | none | A rewrite means a rebase before the merge | Leave as is, or squash at the merge |
| 8 | C4: trust a caller-supplied change set and skip the full-tree scan (−0.17-0.25 s per build) | none | A missed watcher event can leave a stale generation until a periodic reconcile | Decide the reconcile cadence first |
| 9 | C2 step 2: read `.git/HEAD` by hand instead of spawning git | none | Security-adjacent (hostile repo config, the CVE-2024-32465 note in dc-proc) | Only with a security-impact statement |
| 10 | Corpus for the Medium bar | none | Build or freeze a corpus matched for call volume (about 150k calls) | Yes, before declaring 512 MB unreachable |
| 11 | M8: shrink the persist buffers (64 MiB payload window and 64 MiB cache) | none | −50-100 MB peak, at an untested cost to write throughput | Measure first |
| 12 | tree-sitter-rust 0.24 | New dependency version | Possible change in extraction | Your call; needs an extraction parity run |

## Cross-app carry-over (owed once this lands)

- **GitPulse's vendored crates still carry `MAX_BUDGET_RETRIES`.** Re-vendor with `node scripts/vendor-crates.mjs`, then run `check:vendor-schema`. GitPulse's vendored DevMap is at 782b9f16. Schema 29 / MIN_READER 29 already strands the installed devmap and that vendored copy, so `bun run check:schema-parity` is owed independently of this round.
- **GitPulse runs DevMap builds in the macOS background band (O1).** That starvation is what made the budget bug visible in production.
- **Uncommitted files belong to other sessions, not this branch.** The query lane's A/B files (`rust/devmap-query/examples/cli_query_ab.rs`, `benchmarks/query_budget_ab.sh`) are gitignored instead of committed. The swarm's `cas.rs` content-address key omits the extractor version.
