# DevMap Medium-repository targets, measured by the soak (2026-11-01)

Harness: `rust/tools/soak` (crate `devmap-soak`), branch `devmap-perf2/targets`, built from
`claude/devmap-perf-main` 213772a4 plus the soak changes. Corpus: the frozen GitPulse tree
(52,425 symbols, 3,072 files), copied fresh per run into a private directory. Each `.log.txt` in this
directory is the soak's own output (renamed from `.log`, which the repository ignores); each `.csv` is its per-cycle samples
(`cycle,rss_bytes,db_bytes,build_ms,dirty_build_ms,dirty_rss_bytes`).

Targets (spec section 6.6, adopted by the owner): one-file incremental build p95 <= 120 ms;
peak RSS < 512 MB; 1 writer + 8 readers for 10 min with 0 SQLITE_BUSY.

## What is measured

- Wall time is the parent-side time of `/usr/bin/time devmap build .` (process start-up, the
  writer-lock wait, extraction through commit, and exit). The edited build (a function appended
  to one `.ts` file) and the restored build (the file put back) are both one-file incremental
  builds; they are reported separately and pooled (N = 2 x cycles). p95 is nearest-rank.
- Peak RSS is `/usr/bin/time`'s maximum resident set size of that build.
- The corpus has no `.py` file, so the soak's churn target falls back to the first `.ts` file
  (`src/...`), in which it appends `function _soak_N(): number`. Python still wins when present.

## Build mode, 25 cycles (N=25 per kind, N=50 pooled), host load 11-20 (agent swarm)

| run | binary | pooled min | median | p95 | max | incr. peak RSS (min-max) | cold build wall / peak RSS | load avg 1-min start -> end |
|---|---|---|---|---|---|---|---|---|
| m1 | merged (this branch) | 5,858 ms | 7,562 | 9,173 | 10,416 | 1,737-1,757 MiB | 17.2 s / 2,060 MiB | 16.1 -> 13.6 |
| x1 | devmap-main 7926697d | 5,303 ms | 8,442 | 10,310 | 12,628 | 1,782-1,790 MiB | 20.0 s / 2,080 MiB | 12.0 -> 16.7 |
| m2 | merged | 2,328 ms | 6,309 | 8,141 | 8,913 | 1,737-1,754 MiB | 21.7 s / 2,081 MiB | 18.1 -> 19.8 |
| x2 | devmap-main | 2,409 ms | 2,875 | 6,321 | 8,214 | 1,780-1,793 MiB | 16.1 s / 2,099 MiB | 19.8 -> 12.7 |

Order was m1, x1, m2, x2 (sequential, not interleaved within a run).

- Target 1 (p95 <= 120 ms): not met on any run. The best pooled minimum anywhere is 2.3 s, about
  19x the target; the best p95 is 6.3 s. (The same-binary spread, x1 median 8.4 s vs x2 median
  2.9 s, is 2.9x, so the merged-vs-main wall time gap is NOT resolvable on this host.)
- Target 2 (peak RSS < 512 MB): not met. Incremental builds peak at about 1.74 GiB (merged) /
  1.78 GiB (main), 3.4x the target and about 83% of the cold build's 2.06-2.10 GiB. The
  incremental RSS gap between binaries (about 2.4% lower on merged) does not overlap across
  the two pairs of runs (merged 1,737-1,757 vs main 1,780-1,793 MiB) but is tiny against the
  gap to the target.
- Contrast: the 5-cycle run on `rust/testdata` (tiny repo, `verify.sh`'s form) gives 93-118 ms
  builds and 52 MiB; its p95 prints "not established (N=5)" and the 120 ms line is not called met.

## Readers mode, 8 readers x 600 s requested (`r1`), merged binary

One writer: 42 edit -> build -> restore -> build cycles (84 builds) in 609.6 s. Eight reader
threads, each issuing `devmap --json --db <store> search|impact|explore` as a subprocess per
query back to back (30 s timeout per query).

- 3,299 queries, 3,299 ok, 0 SQLITE_BUSY/locked, 0 other errors, 0 timeouts; each reader ran
  407-417 queries. Verdict: `SOAK READERS OK`. Target 3 (0 SQLITE_BUSY): met for this run.
- Caveat on "overlapped": all 3,299 queries were in flight while a build process was running
  (builds take about 7 s back to back), but a build holds the SQLite write lock only during its
  persist phase, so this does not mean every query met a write transaction.
- Reader latency under the 8-way load: median 1,283 ms, p95 3,660 ms, max 8,136 ms (host load
  14.5 -> 20.0). Writer builds under readers: median 7.1 s, p95 10.3 s.
- Run once (N=1 run): an absence of errors in one 10-minute run bounds the rate, it does not
  prove zero.

- "0 SQLITE_BUSY" counts errors that reached the caller. The store's 5 s `busy_timeout`
  (the constant at `devmap-store/src/db.rs:2487`, applied in `db/open.rs`) absorbs a transient BUSY as latency, so the reader
  p95/max above cannot be split between lock waits and CPU contention by this harness.
- Pooled N is 2 x cycles: a 10-cycle run already prints a pooled p95 (N=20) while the per-kind
  lines still say "not established". `verify.sh`'s 5 cycles stay under both.
- Binaries used for the numbers were built before the final commit, which adds a 900 s bound
  (process-group kill) to every measured build; that commit changes no measured path's logic.

## Not established

- Why the build takes 2.3+ s (phase attribution): the soak does not capture per-phase timings.
- Whether any difference in wall time between merged and main exists (load noise dominates).
