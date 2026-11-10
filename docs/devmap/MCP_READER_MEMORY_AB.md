# MCP reader: what the smaller cache and idle eviction buy, and cost

Measured 2026-11-10 on `perf1109/mcp-reader` against its merge-base
(`c2348467`), with `rust/devmap-serve/examples/mcp_ab_bench.rs`. Both arms are
release builds of `devmap`, driven as real `devmap mcp --db <store>` processes
over stdio.

| | |
|---|---|
| Store | a copy of a real store: 389,332,992 bytes (371 MiB), 58,366 symbols, 218,602 edges, generation 4825, schema 29 |
| Calls per process (N) | 40: the first is `devmap_impact` on target 1, then `devmap_status`, `devmap_impact` on targets 2, 3, 1, ... cycling |
| Rounds | 23 run, the first 2 discarded, **n = 21** reported |
| Order | which arm runs first alternates every round (seed 1 picks round 0) |
| "Cold" | a new process with empty SQLite and edge-index caches over a warm OS file cache: the store file is read in full before each spawn, so both arms start identically |
| "After idle eviction" | the process sat idle 660 s (the idle TTL is 600 s, the sweep 30 s) before the figure is read or the query sent |
| Load (1-minute) | real pair 9.09 at start, 5.02 at end; null pair 4.76 at start, 3.97 at end |

Memory is read by `ps -o rss` and by `vmmap --summary` (`Physical footprint`).
After a long idle the OS compresses a process's pages, so RSS collapses
(base: 12.8 to 200 MiB across rounds) and says little; the footprint is the
figure to read for the idle window. Base never evicts, and its footprint still
fell 52 MiB over the idle window (216.4 to 164.3 MiB) for that reason, so the
eviction's own share of the saving is smaller than the headline difference.

## Result

Median [min, max] per arm; the paired difference is branch minus base per
round (lower is better); the null pair is the branch binary run as both arms.

| Quantity | base | branch | paired diff, median (branch lower in) | null pair diff, median (B lower in) |
|---|---|---|---|---|
| RSS after N queries (MiB) | 228.52 [223.08, 233.42] | 164.66 [159.72, 178.73] | **-63.27** (21/21) | -1.19 (13/21) |
| Footprint after N queries (MiB) | 216.40 [211.00, 221.30] | 152.70 [147.80, 166.80] | **-63.10** (21/21) | -1.20 (13/21) |
| RSS after idle window (MiB) | 109.55 [12.77, 200.16] | 91.42 [44.34, 134.28] | -14.75 (14/21), unreliable | 0.00 (10/21) |
| Footprint after idle window (MiB) | 164.30 [158.50, 177.70] | 91.00 [81.30, 102.20] | **-73.20** (21/21) | -0.60 (12/21) |
| First query, cold reader (ms) | 167.03 [139.07, 212.47] | 155.41 [147.87, 184.99] | -0.34 (11/21) | -0.25 (11/21) |
| First query after idle window (ms) | 38.52 [35.50, 42.26] | 138.17 [133.26, 160.77] | **+99.29** (0/21) | +0.06 (10/21) |
| Warm `impact`, per-process median (ms) | 37.40 [32.96, 43.45] | 36.08 [32.93, 41.44] | -0.36 (14/21) | -0.16 (15/21) |
| Warm `status`, per-process median (ms) | 125.89 [107.22, 145.92] | 134.82 [122.08, 159.09] | **+15.75** (2/21) | -0.87 (14/21) |

Null pair arms (same binary), median [min, max]:

| Quantity | A | B |
|---|---|---|
| RSS after N (MiB) | 162.69 [156.70, 167.06] | 161.55 [157.41, 166.94] |
| Footprint after N (MiB) | 151.20 [145.20, 155.50] | 150.00 [145.90, 155.40] |
| RSS after idle window (MiB) | 43.88 [43.73, 44.05] | 43.84 [43.73, 43.95] |
| Footprint after idle window (MiB) | 71.70 [66.20, 73.90] | 70.40 [66.30, 74.20] |
| First query, cold (ms) | 143.05 [136.49, 202.05] | 141.75 [136.13, 161.27] |
| First query after idle window (ms) | 137.83 [134.25, 144.82] | 138.02 [135.20, 149.94] |
| Warm `impact` (ms) | 33.23 [32.55, 39.63] | 33.10 [32.28, 37.34] |
| Warm `status` (ms) | 102.61 [99.02, 125.30] | 102.09 [97.19, 110.39] |

Every null difference is inside its arms' own spread, so the run is valid.
Absolute levels drift between the two runs (the same binary's warm `status`
was 102 ms in the null run and 135 ms in the real run), so only differences
within one run are compared.

## What it says

- **Memory saved:** about 63 MiB of footprint (29%) on a reader that has served
  40 queries, in all 21 rounds and 50 times the null difference. Over an idle
  window of 11 minutes the branch holds about 73 MiB less (91 against 164 MiB),
  of which roughly 10 MiB is the eviction beyond what the OS compression gave the
  base for free (inferred from the two arms' drops over the window, 61.7 and
  52.1 MiB; the compression is not measured separately).
- **Cost of the saving:** the first query after an eviction takes 138 ms where
  a warm reader takes 39 ms: about +99 ms, in every round, the same as the
  branch's own cold first query (138 to 155 ms). A reader nobody used for ten
  minutes pays one cold query.
- **Cold first query:** no change (-0.34 ms against a null of -0.25 ms and a
  spread of tens of ms).
- **Warm `impact`:** no change (-0.36 ms; null -0.16 ms).
- **Warm `status`: regressed, about +16 ms (+12%)**, the branch slower in 19 of
  21 rounds against a null of -0.87 ms. `status` reads many pages that a
  16 MiB cache does not hold, so it rereads them from the OS cache each call;
  `impact` is served from the in-memory edge index and is unaffected. This was
  not part of the earlier claim; it is a cost of `MCP_READER_MEMORY` and the
  card's "must not regress" is not met for `status`.

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
below a 1-minute load of 30. The earlier in-process driver
(`mcp_reader_memory_bench.rs`) measures memory settings only, not the shipped
binaries.
