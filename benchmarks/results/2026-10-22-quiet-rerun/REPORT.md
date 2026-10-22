# Quiet re-run of the 2026-10-20/21 DevMap benches — 2026-10-22

Measured by lane A of the 2026-10-22 wave. The raw logs are kept out of git (ft-bc87301a).

## Arms

- **main:** `f796dd8c`.
- **base:** `aacffb85`, which is main before the run, built from `git archive`.
- **jvm:** `a247f398` (`lane/jvm-scope-model`, unmerged). Its parent is `99d41a1f`, so a jvm-vs-main comparison also carries main's later resolver changes.
- **mbres:** `99d41a1f` plus `resolve_bench.rs`.
- **mainres:** `f796dd8c` plus the same example.

`devmap --version` prints the enclosing checkout's sha for every scratch build, so the arms were told apart by source tree and binary sha256 (main `7ca83a5e…`, base `dac6574e…`).

## The machine was never fully quiet

- Other sessions held the cargo slot gate throughout.
- Load at run start ranged 12–201 depending on the block. Each table names its load.
- **Every verdict rests on instructions retired.** Their spread was 0.3–2.1% even at load 16–92.
- **Wall time is not a basis for any verdict:** cold-build spread ran 72–134% of the min.

## Method

| Item | Detail |
|---|---|
| Corpus | One copy of `rust/` at `f796dd8c`: 1,041 files, 17,595 symbols, 69,740 edges. |
| Runs | Interleaved A/B with alternating order. min-of-7 per arm unless stated. |
| (a) Full build | `devmap build <corpus> --full --db <fresh> --json` |
| (b) One-file edit | Copy the arm's cold store, append one two-line function to `dc-glob/src/lib.rs`, run `--affected dc-glob/src/lib.rs`. |
| (c) Queries | `dead --json` and `impact 'devmap-store/src/db.rs::Store' --json` against the arm's own cold store, as whole-process figures. |
| (d) Resolver | `resolve_bench` on ChronosFlow (1,330 files, 16,281 symbols, 68,910 edges). All arms index the same counts. |

## Summary

Instructions, main vs base, except (d), which is jvm vs main.

| Cell | Load | main | base | Δ (min) | Verdict |
|---|---|---|---|---|---|
| (a) full build, run 2 | 16.5–21.3 | 193.39 G | 191.20 G | +1.15% (median +1.23%) | About +1.2% instructions on main; wall cannot resolve it |
| (a) full build, run 1 | 16–92 | 194.07 G | 191.83 G | +1.17% | same |
| (b) one-file `--affected`, run 2 | 13.6–16.8 | 30.94 G | 30.89 G | +0.17% (median +0.04%) | no difference |
| (b) one-file `--affected`, run 1 | 29–83 | 31.28 G | 31.26 G | +0.06% | no difference |
| (c) `dead --json`, n=21 | 17.9–20.3 | 0.3726 G | 0.3711 G | +0.4% | no difference |
| (c) `impact …::Store`, n=21 | 17.9–20.3 | 0.8539 G | 0.8534 G | +0.06% | no difference |
| (d) resolver per round, jvm vs main | 42–97 | jvm 23.25 G | main 23.14 G | +0.46% (median +0.35%) | the jvm tables cost no measurable time |

Against its own parent (`mbres`), the jvm lane is +0.38% (min) and +0.30% (median).

## (a) Full build, run 2 (n=14)

Load min 16.5, median 19.2, max 21.3.

| Metric | base min / median / max | main min / median / max |
|---|---|---|
| Instructions (G) | 191.205 / 191.533 / 192.492 | 193.392 / 193.889 / 194.382 |
| Wall (s) | 5.08 / 7.64 / 11.89 | 5.59 / 6.82 / 9.61 |
| Peak footprint (MB) | 768.8 / 776.2 / 789.5 | 778.3 / 784.2 / 795.1 |

`persist:write` from the build's own `--json` (min / median): base 2.056 / 2.693 s, main 2.153 / 2.833 s. The unattributed `other` residual is 44–57% of `persist:write` on both arms; see the store write-cost proposal.

## (b) One-file `--affected`, run 2 (n=14)

Load 13.6–16.8.

| Metric | base min / median / max | main min / median / max |
|---|---|---|
| Instructions (G) | 30.885 / 30.978 / 31.220 | 30.937 / 30.991 / 31.167 |
| Wall (s) | 2.76 / 3.21 / 3.90 | 2.79 / 3.04 / 3.93 |
| Peak footprint (MB) | 692.1 / 694.0 / 696.5 | 692.1 / 696.7 / 698.9 |

## (c) Queries (n=21)

Load 17.9–20.3. The wall clock resolves only to 10 ms.

| Cell | base min / median / max (G) | main min / median / max (G) |
|---|---|---|
| `dead --json` | 0.3711 / 0.3817 / 0.4625 | 0.3726 / 0.3832 / 0.4152 |
| `impact …::Store --json` | 0.8534 / 0.8564 / 0.8920 | 0.8539 / 0.8552 / 0.8607 |

## (d) JVM tables on ChronosFlow

**How the per-round figure is computed:** the instruction difference between a 1-round and a 6-round process (n=5 each, interleaved across the three arms), divided by 5. This is one index+resolve round with untimed extraction cancelled.

| Arm | 1 round min (G) | 6 rounds min (G) | per round min / median (G) |
|---|---|---|---|
| jvm `a247f398` | 85.056 | 201.296 | 23.248 / 23.231 |
| mainres `f796dd8c` | 85.137 | 200.847 | 23.142 / 23.149 |
| mbres `99d41a1f` | 85.150 | 200.950 | 23.160 / 23.162 |

- **Wall time is unusable.** Per-arm spread was 119–190% at load 12–20. A second pass ran while load climbed to 201 and was discarded.
- **Tables in-process.** The lane's own measurement (+1.2–1.9% of index on a quiet machine, +12% on a 30k-method file) is recorded in `resolve_bench.rs` on that branch.

## Not measured

- The CodeGraph comparison figures from 20261007 cover the whole repository; this corpus is `rust/` only.
- The 30k-property Swift ≤1.3× instruction target against `9030d08c`; no build of `9030d08c` was made.
- A wall-clock A/B at load below 10.
