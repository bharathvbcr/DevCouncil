# MCP reader: what the smaller cache and idle eviction buy, and cost

Measured 2026-11-10 on `perf1109/mcp-reader` against its merge-base
(`c2348467`), with `rust/devmap-serve/examples/mcp_ab_bench.rs`. Both arms are
release builds of `devmap`, driven as real `devmap mcp --db <store>` processes
over stdio. The branch arm, and the binary run as both null arms, was built from
the tree at `c5ca3495`: the `status` fix `7f8ee6b5` plus one example file that is
not part of the binary. **It predates the hardening of the memo described under
"The memo" below (own-write stamp and age bound); those changes were tested but
not benchmarked.** The base arm was built from `c2348467`.

| | |
|---|---|
| Store | a copy of a real store: 389,332,992 bytes (371 MiB), 58,366 symbols, 218,602 edges, generation 4825, schema 29 |
| Calls per process (N) | 40: the first is `devmap_impact` on target 1, then `devmap_status`, `devmap_impact` on targets 2, 3, 1, ... cycling |
| Rounds | 23 run, the first 2 discarded, **n = 21** reported |
| Order | which arm runs first alternates every round (seed 1 picks round 0) |
| "Cold" | a new process with empty SQLite and edge-index caches over a warm OS file cache: the store file is read in full before each spawn, so both arms start identically |
| "After idle eviction" | the process sat idle 660 s (the idle TTL is 600 s, the sweep 30 s) before the figure is read or the query sent |
| Load (1-minute) | real pair 17.07 at start, 7.32 at end; null pair 6.98 at start, 13.18 at end (all below 30) |

Memory is read by `ps -o rss` and by `vmmap --summary` (`Physical footprint`).
After a long idle the OS compresses a process's pages, so RSS collapses and says
little; the footprint is the figure to read for the idle window. Base never
evicts, and its footprint still fell 78 MiB over the idle window (211.8 to 133.8
MiB) for that reason, so the eviction's own share of the saving is much smaller
than the headline difference.

## Result

Median [min, max] per arm; the paired difference is branch minus base per round
(lower is better); the null pair is the branch binary run as both arms (B minus
A).

| Quantity | base | branch | paired diff, median (branch lower in) | null pair diff, median (B lower in) |
|---|---|---|---|---|
| RSS after N queries (MiB) | 223.59 [218.78, 230.67] | 162.73 [158.28, 171.44] | **-61.73** (21/21) | -0.59 (14/21) |
| Footprint after N queries (MiB) | 211.80 [207.00, 218.90] | 151.10 [146.60, 159.70] | **-61.70** (21/21) | -0.60 (11/21) |
| RSS after idle window (MiB) | 107.89 [16.11, 193.92] | 83.03 [54.39, 107.62] | -22.12 (16/21), unreliable | 0.11 (10/21) |
| Footprint after idle window (MiB) | 133.80 [130.00, 147.30] | 67.60 [43.30, 71.60] | **-66.10** (21/21) | -0.20 (11/21) |
| First query, cold reader (ms) | 148.09 [139.41, 190.84] | 152.44 [138.53, 275.93] | +1.94 (7/21) | -8.13 (12/21) |
| First query after idle window (ms) | 49.73 [42.80, 75.98] | 201.98 [166.32, 252.00] | **+148.75** (0/21) | -6.18 (13/21) |
| Warm `impact`, per-process median (ms) | 35.30 [33.45, 39.41] | 36.23 [33.48, 49.88] | +0.61 (7/21) | -0.06 (11/21) |
| Warm `status`, per-process median (ms) | 99.55 [92.93, 113.64] | 71.54 [67.88, 98.89] | **-24.44** (20/21) | +1.10 (9/21 B lower) |

Null pair arms (same binary), median [min, max]:

| Quantity | A | B |
|---|---|---|
| RSS after N (MiB) | 162.91 [158.98, 176.84] | 161.78 [144.70, 171.31] |
| Footprint after N (MiB) | 151.20 [147.30, 165.10] | 151.30 [145.00, 159.60] |
| RSS after idle window (MiB) | 44.59 [12.89, 109.23] | 44.52 [12.86, 122.58] |
| Footprint after idle window (MiB) | 75.00 [70.70, 98.40] | 74.60 [68.90, 95.00] |
| First query, cold (ms) | 202.78 [159.68, 303.42] | 206.84 [160.39, 253.93] |
| First query after idle window (ms) | 188.04 [171.73, 211.24] | 185.27 [163.35, 208.46] |
| Warm `impact` (ms) | 50.56 [37.94, 64.22] | 49.70 [35.16, 68.28] |
| Warm `status` (ms) | 98.76 [73.63, 124.95] | 98.28 [75.20, 136.32] |

Every null difference is inside its arms' own spread, so both runs are valid.
Absolute levels differ between the two runs: the same binary's warm `impact` was
50 ms in the null run and 36 ms in the real run, its warm `status` 98 and 72 ms.
The 1-minute load figures at the ends of the runs (real 17.07 to 7.32, null 6.98
to 13.18) do not explain it: the null run started at the lower load. Measured:
the load at the two ends of each run. Inferred, not measured: other work on the
host during the null run (a 1-minute load is a poor sample of a 15-minute run).
Only differences within one run are compared.

## What it says

- **Memory saved:** about 62 MiB of footprint (29%) on a reader that has served
  40 queries, in all 21 rounds and about 100 times the null difference. Over an
  idle window of 11 minutes the branch holds about 66 MiB less (68 against 134
  MiB).
- **Warm `status` no longer regresses; it is 24 ms faster than base** (71.5
  against 99.6 ms, 20 of 21 rounds, null +1.10 ms). See below for why it is
  faster rather than merely equal.
- **Cold first query:** no change (+1.94 ms against a null of -8.13 ms and a
  spread of tens of ms).
- **Warm `impact`:** no change (+0.61 ms; null -0.06 ms).
- **The eviction trade, stated plainly.** After 660 s idle the next query costs
  about 149 ms more than on base in this run (202 against 50 ms; 99 ms more in an
  earlier run, 138 against 39 ms: the figure moves with how much of the store
  the OS still holds in its file cache). For that, idle eviction itself returns
  only about 5 to 10 MiB beyond what the OS compression gives base for free: the
  base footprint falls 78 MiB over the window on its own, the branch's 84 MiB
  (inferred from the two arms' drops, 211.8 to 133.8 and 151.1 to 67.6; the
  compression is not measured separately). The other ~62 MiB of the saving is
  the smaller steady-state footprint (16 MiB page cache instead of 64 MiB), which
  costs nothing at query time. The steady-state saving is the win.
  **Recommendation: keep idle eviction, but treat it as the minor part of this
  change.** It costs one cold query per ten idle minutes and frees only the
  single-store 5 to 10 MiB shown here; its value grows with the number of stores
  one reader process holds (each evicted store frees its own edge index), which
  this single-store benchmark does not measure. If the 100 to 150 ms first query
  after a break matters more than that, drop the idle TTL and keep the cache
  change; the data does not make the eviction clearly not worth keeping, so it
  stays and the decision is the owner's.

## Why `status` regressed, and what fixed it

A first run of this benchmark, with the cache change alone, showed warm `status`
at +15.75 ms (126 to 135 ms, the branch slower in 19 of 21 rounds, null
-0.87 ms). The cause, found by timing each stage of one warm `status` on the same
store at 16 MiB and 64 MiB (one cache size per process, alternated;
`mcp_status_stages.rs`):

- `status` re-read whole-generation rows on every call that nothing had changed:
  the payload-identity scan of `generation_files`, the file-hash map the
  freshness walk compares, the edge-confidence count over all 218,602 edges
  (14 to 18 ms alone), and a payload-encoding count that reads past each
  payload's blob to its next column. A 64 MiB cache kept those pages and hid the
  repetition; a 16 MiB cache rereads them from the OS each call.
- `status` also took three *full* snapshots (full-text probe, coverage gaps,
  encoding count) where two of them only compared the generation id and the
  pending count to the first snapshot's.
- The cache was not the defect: `scan_tree` (the filesystem walk, about 55 ms,
  no store) is cache-independent, and the cache-sensitive stages were exactly
  those repeated scans.

The fix reads only the two fields in the re-checks, and holds the four repeated
answers per `(generation, PRAGMA data_version)`. The generation id alone is not a
complete key for them: a payload identity can be rewritten in place under a
standing id (an existing test does exactly that), so a commit by any other
connection ends every held answer. Base pays those scans on every call, which is
why the branch is now faster than base and not merely equal. A counting test
(`one_status_takes_one_full_snapshot`) and four view-dropping tests assert the
second warm call runs no SQL on the held tables; the stage example reproduces the
breakdown.

## The memo

`status` holds four answers on the `Store`: the payload-identity check, the
file-hash map the freshness walk compares, the edge-confidence count and a clean
payload-encoding verdict (a fault is never held). Each is keyed on the
generation, on SQLite's `PRAGMA data_version` (moves when another connection
commits) and on this connection's own `total_changes()` (moves on every row the
store's own writes touch, so no write path has to remember to end the memo: the
key has one owner, `Store::connection_stamp`). Each is also bounded in age at
60 seconds, because page damage or an in-place overwrite of the file moves none
of those keys and a pass read once would otherwise be reported for the life of
the process; at 60 s a health poll pays the scan (about 16 ms) at most once a
minute. How often hooks call `status` was not measured. The file-hash map is
not capped: one path string and one hash per indexed file, about 0.4 MB at 3,320
files (estimated, not measured; an estimated 10 to 15 MB at 100,000 files), and
it lives as long as the `Store`, so a writable daemon holds it for the life of
the daemon. Callers of `latest_generation_payload_is_current` and
`edge_confidence_mismatches` outside this crate (GitPulse's vendored copy,
Manvi's linked one) get the same memo: a repeat within a minute returns the held
answer unless a write moved a key.

## Re-timing at the tip (`51af7891`, after the hardening)

Measured 2026-11-10/11, `mcp_ab_bench` with a fresh copy of the current GitPulse
store (same size, 389,332,992 bytes; not the same snapshot as above), base
`c2348467` against the tip, n = 21 (23 rounds, 2 discarded), one untimed warm-up
per arm, `--evict-wait-secs 1` (the idle-window rows of these runs mean nothing
and are not reported). The null pair is the tip binary as both arms. 1-minute
load: null pair 24.82 at start, 12.97 at end; real pair 12.97 at start, 18.53 at
end (all runs below 30 at both ends). Medians; paired difference is tip minus
base, null is B minus A.

| Quantity | base | tip | paired diff (tip lower in) | null diff (B lower in) |
|---|---|---|---|---|
| Warm `status` (ms) | 130.31 [101.76, 191.66] | 99.27 [77.76, 207.45] | **-27.92** (18/21) | -1.24 (14/21) |
| Footprint after 40 queries (MiB) | 217.20 [191.50, 225.90] | 156.30 [118.20, 167.40] | **-61.50** (21/21) | -0.70 (11/21) |
| Cold first query (ms) | 184.24 [153.91, 512.13] | 184.33 [162.15, 467.19] | +6.36 (8/21) | -1.92 (11/21) |
| Warm `impact` (ms) | 43.12 [36.23, 51.26] | 44.07 [38.36, 82.39] | +1.65 (6/21) | -0.86 (15/21) |

The null pair is near zero on every row, so the runs are valid. Warm `status`
is still about 28 ms faster than base after the hardening, and the footprint
saving is unchanged. Cold first query (+6 ms against arm spreads of 100 ms or
more) and warm `impact` (+1.65 ms, tip higher in 15 of 21 rounds, null -0.86)
did not clearly move; a small `impact` slowdown is not excluded.

**Not measured.** The `status` call after more than 60 s of real idle (the
re-scan after the held answers age out), for the pair and its null pair: the
host load stayed above 30 for the 2-hour window after the first two runs (the
idle-gap null pair ended at load 65.9 and was invalid; its retry started at 26.6
and was stopped at load above 50), so no valid run exists. `--aged-wait-secs` in
`mcp_ab_bench` is committed for that run. That the held-answer path was taken is
inferred from the 28 ms drop against base, not proven by a counter; the
unit tests (`generation_checks_memo`) prove the path in isolation.

## Reproduce

```text
mcp_ab_bench --store <copy.sqlite> --rounds 23 --discard 2 --seed 1 \
    --evict-wait-secs 660 --queries 40 \
    --target src-tauri/src/engine/git_cli.rs::validate_repo \
    --target src-tauri/src/ledger/mod.rs::append \
    --target src-tauri/src/engine/git_reader.rs::git_in \
    --arm base=<devmap at c2348467> --arm branch=<devmap at the branch head>
```

Null pair: `--arm nullA=<branch devmap> --arm nullB=<branch devmap>`. Start only
below a 1-minute load of 30. Stage breakdown of one warm `status`:
`mcp_status_stages <store.sqlite> 15 <cache_kib>`, one cache size per process.
The earlier in-process driver (`mcp_reader_memory_bench.rs`) measures memory
settings only, not the shipped binaries.
