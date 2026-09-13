# DevMap against five code-graph tools, across four repositories

- generated: 2026-09-13 UTC
- host: Apple M5 Pro (Mac17,8), 18 cores, macOS Darwin 27.0.0
- statistic: **minimum of 3 interleaved repeats** (median and max in `summary.json`)
- raw data: [`measurements.jsonl`](measurements.jsonl), [`comparison.json`](comparison.json), [`gortex-gates.jsonl`](gortex-gates.jsonl)

Every earlier competitor report in this directory measures one frozen DevCouncil
corpus. This run measures **four different repositories**, to see whether the
ordering between tools holds as corpus size and language mix change. It does
not replace those reports: it has no caller-accuracy checks, no persistent-MCP
timings, and no stale-index recovery scenarios.

## What was measured

| tool | version | invocation |
|---|---|---|
| DevMap | 0.2.1 build `9d8e6db` | `devmap --db … build <corpus>` |
| CodeGraph | v1.6.0 | `codegraph init` / `codegraph sync` |
| CBM (codebase-memory-mcp) | v0.10.8 | `cli index_repository --persistence false` |
| GitNexus | 1.6.9 | `gitnexus analyze --index-only` |
| Graphify | 0.9.59 | `graphify extract --code-only --no-cluster` |
| Gortex | v0.64.3 | resident daemon, measured at its own log gates |

Corpora are APFS clones of snapshots pinned at each repository's HEAD. Each tool
gets its **own** copy, because these tools write index state into or beside the
tree. No working checkout and no live product index was read or written.

| repo | commit | files DevMap indexed | symbols | edges |
|---|---|---|---|---|
| DevPrism | `feaef2ffd27f` | 595 | 6,290 | 18,534 |
| DevCouncil | `9d8e6db8b671` | 1,077 | 11,375 | 37,055 |
| GitPulse | `bbafcb892e8a` | 1,992 | 27,834 | 91,006 |
| scholarlm | `7bb95186c5d4` | 4,324 | 41,900 | 145,699 |

File counts differ per tool — Gortex reindexed 4,414 files on scholarlm where
DevMap indexed 4,324 — so throughput is not comparable between tools.

## Cold index

Minimum seconds; `×N` is times DevMap's, from unrounded values.

| repo | DevMap | CodeGraph | CBM | GitNexus | Graphify |
|---|---|---|---|---|---|
| DevPrism | **0.791s** | 1.863s ×2.4 | 5.796s ×7.3 | 14.549s ×18.4 | 4.804s ×6.1 |
| DevCouncil | **1.973s** | 3.130s ×1.6 | 8.526s ×4.3 | 31.172s ×15.8 | 17.038s ×8.6 |
| GitPulse | **2.948s** | 5.642s ×1.9 | 5.543s ×1.9 | 40.350s ×13.7 | 11.494s ×3.9 |
| scholarlm | **5.600s** | 7.983s ×1.4 | 7.014s ×1.3 | 38.963s ×7.0 | 25.661s ×4.6 |

DevMap had the lowest cold time on all four corpora, by between 1.3× and 18.4×.

CBM barely responds to corpus size — 5.5s on 595 files and 7.0s on 4,324 — so
most of its cold number is fixed process cost, not indexing. Its advantage
narrows against DevMap purely because DevMap's own cost grows with the corpus.

## Unchanged refresh (`warm`)

| repo | DevMap | CodeGraph | CBM | GitNexus | Graphify |
|---|---|---|---|---|---|
| DevPrism | **0.050s** | 0.171s ×3.4 | 5.432s ×109.2 | 0.490s ×9.9 | 2.099s ×42.2 |
| DevCouncil | **0.091s** | 0.221s ×2.4 | 5.579s ×61.2 | 0.631s ×6.9 | 4.698s ×51.5 |
| GitPulse | **0.092s** | 0.195s ×2.1 | 5.486s ×59.7 | 0.692s ×7.5 | 6.709s ×73.0 |
| scholarlm | **0.118s** | 0.393s ×3.3 | 5.631s ×47.9 | 1.112s ×9.5 | 13.392s ×113.9 |

This is the watcher's common tick, and DevMap's largest margin. It is the one
stage where DevMap stays essentially flat as the corpus grows (0.050s → 0.118s
for 7× the files) while Graphify grows 6×.

## One file changed (`touch`)

| repo | DevMap | CodeGraph | CBM | GitNexus | Graphify |
|---|---|---|---|---|---|
| DevPrism | 0.358s | **0.320s ×0.9** | 5.796s ×16.2 | 17.357s ×48.5 | 2.093s ×5.8 |
| DevCouncil | 0.790s | **0.447s ×0.6** | 8.397s ×10.6 | 29.158s ×36.9 | 4.702s ×6.0 |
| GitPulse | 1.371s | **0.392s ×0.29** | 5.468s ×4.0 | 36.878s ×26.9 | 6.740s ×4.9 |
| scholarlm | 3.085s | **0.786s ×0.25** | 6.943s ×2.3 | 71.048s ×23.0 | 14.083s ×4.6 |

**This is the one stage DevMap loses, and the gap widens with corpus size**:
CodeGraph is 0.9× DevMap on the smallest repository and 0.25× on the largest —
DevMap is **3.9× slower than CodeGraph** on scholarlm.

The cause is known and measured independently the same day. The
[map benchmark](../../map/20260913T195023Z.md) broke a scholarlm `touch` build
into phases: persistence falls to 29% of cold and extraction to 36%, but
`resolver:index` stays at 97%, `resolving` at 94% and `analyzing edges` at 100%.
Roughly 59% of an incremental build is whole-tree work that does not shrink with
the change set. `benchmarks/README.md` records this as deliberate — resolution
"covers the whole tree so liveness and community detection mean the same thing
on both paths" — but its consequence is this column. Two independent
measurements, one internal and one competitive, agree.

GitNexus is slowest here because the run used its full `analyze` path rather
than an incremental one; that is this harness's choice, not a GitNexus limit.

## Gortex (resident daemon — not comparable to the table above)

Gortex holds a daemon rather than exiting, so a CLI wall-clock wrapper would
measure the wrong thing. These are its own log gates, **n=1**.

| repo | query-ready | enrichment complete | files reindexed | database |
|---|---|---|---|---|
| DevPrism | 7.353s | 14.142s | 758 | 186 MB |
| DevCouncil | 14.505s | 33.786s | 1,345 | 394 MB |
| GitPulse | 30.589s | 84.489s | 2,061 | 699 MB |
| scholarlm | 122.046s | 332.077s | 4,414 | 2,327 MB |

`query-ready` is the earliest point queries are served; enrichment continues
behind it. The scholarlm run recorded a 1-minute load average of 16.5, so treat
it as an upper bound. Against a resident daemon, a standalone CLI pays process
startup on every call that Gortex pays once — the reason these sit in their own
table.

## Peak memory, cold index (sampled process tree, MB)

| repo | DevMap | CodeGraph | GitNexus | Graphify |
|---|---|---|---|---|
| DevPrism | **406** | 925 | 2,919 | 967 |
| DevCouncil | **664** | 2,486 | 3,442 | 3,338 |
| GitPulse | **1,078** | 2,277 | 4,563 | 1,252 |
| scholarlm | **1,676** | 2,902 | 4,958 | 1,329 |

DevMap used the least memory on every corpus. These figures reproduced within
2% across two independent passes.

**CBM is deliberately absent.** Its sampled peak varied by −75% to −96% between
the two passes (1,530 MB then 389 MB on DevCouncil; 1,778 MB then 66 MB on
GitPulse) because its workers are too short-lived for the 50 ms sampler to catch
reliably. A number that unstable is not evidence, so it is not reported.

## Index size on disk (MB)

| repo | DevMap | CodeGraph | Graphify | GitNexus | Gortex |
|---|---|---|---|---|---|
| DevPrism | 93 | **49** | **18** | 254 | 188 |
| DevCouncil | 170 | 72 | **48** | 448 | 394 |
| GitPulse | 333 | 166 | **85** | 647 | 699 |
| scholarlm | 544 | 295 | **167** | 742 | 2,327 |

DevMap's store is 1.8–2.4× CodeGraph's and 3.3–3.5× Graphify's, while sitting at
37–73% of GitNexus's — the margin against GitNexus narrows as the corpus grows
(0.37× on DevPrism, 0.73× on scholarlm), so it should not be read as a fixed
advantage. DevMap's ~50% space overhead is a known, asserted trade: it uses
incremental vacuum rather than a full rewrite, documented in
`benchmarks/README.md`. CBM stores nothing here — it ran with
`--persistence false`.

## How much to trust this

**Two measurement bugs were found and fixed before these numbers.** Both
produced results that looked like *wins*, which is why they are worth stating.

1. **A "cold" index that was not cold.** Competitors write their index inside
   the corpus, which the harness recreates per repeat; DevMap's store is a
   `--db` path in scratch and survived, so its cold stage silently became an
   unchanged-refresh — 0.066s for 595 files. GitNexus had the same exposure via
   `GITNEXUS_HOME`. Out-of-tree index state is now deleted before every cold run.
2. **A Gortex run that indexed nothing.** Without the corpus registered in
   `config.yaml` the daemon warms up over an empty graph and still emits both
   log gates: ~0.4s for every repository, with a byte-identical 552,960-byte
   schema-only database. `gortex_cold.py` now asserts `files_reindexed > 0` from
   the daemon's own warmup summary and fails rather than records. The void run is
   kept as `gortex-gates-untracked-void.jsonl`.

**A third fault was methodological.** The first pass looped
`for tool: for repeat:`, so all three samples of a cell ran back-to-back and one
load excursion contaminated every sample of DevMap-on-GitPulse — a *minimum* of
10.106s against a re-measured 2.948s. Taking the minimum does not rescue a cell
whose every sample is bad. Repeat is now the outermost loop; that pass is kept as
`measurements-pass1-consecutive.jsonl`.

It was caught by a cross-run control: the independent
[map benchmark](../../map/20260913T195023Z.md) had measured DevMap on the same
four corpora hours earlier with a different harness. After the fix, the two
agree within **3.7% on all four repositories**:

| repo | map benchmark | this run | delta |
|---|---|---|---|
| DevPrism | 0.796s | 0.791s | −0.6% |
| DevCouncil | 2.038s | 1.973s | −3.2% |
| GitPulse | 3.060s | 2.948s | −3.7% |
| scholarlm | 5.626s | 5.600s | −0.5% |

Remaining limits:

- **These tools do not do the same work.** Timings describe each tool's own
  native command. Nothing here measures caller accuracy, query quality, or
  symbol-resolution correctness, so no row is a quality ranking.
- **The machine was shared.** A concurrent build and test run held 1-minute load
  between roughly 5 and 18 throughout. Minimums are the defensible statistic;
  per-sample `load_average` is in `measurements.jsonl`.
- **Gortex is n=1** and measured at different gates than everything else.
- **One stage per tool per scenario.** No tool was tuned, and a faster mode may
  exist for any of them — GitNexus's `touch` in particular used its full
  `analyze` path.
- **Snapshots exclude uncommitted work**; three in-flight files in scholarlm were
  not indexed by any tool.

## Reproducing

```bash
python3 make_plan.py                 # pin binaries, snapshots, edit targets
python3 run.py all full 3            # the five-CLI matrix, interleaved
python3 gortex_env.py                # private socket / XDG roots
python3 gortex_cold.py all 1         # daemon gates, with the indexed assertion
python3 analyze.py && python3 finalize.py
```
