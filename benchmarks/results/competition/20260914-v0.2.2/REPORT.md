# DevMap v0.2.2 against five code-graph tools, across four repositories

- generated: 2026-09-14 UTC
- host: Apple M5 Pro (Mac17,8), 18 cores, macOS Darwin 27.0.0
- statistic: **minimum of 3 interleaved repeats** (median and max in `summary.json`)
- DevMap: `0.2.2 (store schema 22, code graph schema 2, build 1d680486869d)` — the `v0.2.2` tag
- raw data: [`measurements.jsonl`](measurements.jsonl), [`ab-measurements.jsonl`](ab-measurements.jsonl), [`comparison.json`](comparison.json), [`summary.json`](summary.json), [`gortex-gates.jsonl`](gortex-gates.jsonl)

This repeats the [20260913-multirepo](../20260913-multirepo/REPORT.md) four-repository
matrix against the shipping v0.2.2 binary, and adds the thing that report could not
provide: a **controlled A/B between v0.2.1 and v0.2.2** on the same corpora in the
same session. All 180 matrix measurements succeeded; none were discarded.

It covers both halves of the earlier reports: the four-repository **indexing
latency, memory and store size** matrix from 20260913-multirepo, and the
**correctness and query** campaign from
[build-48cd3c7](../20260913-48cd3c7/REPORT.md) — exact definition lookup, caller
accuracy against source-inspected pairs, edit visibility, index staleness, query
latency and coverage inventories.

Two things remain unmeasured, as in every prior report: **persistent-MCP
throughput** for the tools that offer one, and any **end-to-end coding-agent
outcome**. Nothing here says whether an agent ships better code.

## Tool provenance — read this before comparing to the prior report

A 2026-09-13 cleanup pass deleted `.devcouncil/benchmarks/`, taking every pinned
competitor executable with it. They were reinstalled from their original release
URLs and **each was verified byte-for-byte against the sha256 the build-48cd3c7 run
recorded** before being used. The digests are in
[`plan.json`](plan.json); the installer that checked them is
[`install_tools.py`](install_tools.py), and it treats a mismatch as a failure
rather than a warning.

So the competitor *builds* are identical to the prior report's. The *corpora* are
not: three of the four repositories advanced past the commits that report measured.
The movement is small (0.2–2% of files), but it is not zero, and load conditions
differed between the two sessions. **Competitor numbers here are therefore not a
controlled A/B against 20260913-multirepo** — only the v0.2.1/v0.2.2 section below is.

| tool | version | invocation |
|---|---|---|
| DevMap | 0.2.2 build `1d680486` | `devmap --db … build <corpus>` |
| CodeGraph | v1.6.0 | `codegraph init` / `codegraph sync` |
| CBM (codebase-memory-mcp) | v0.10.8 | `cli index_repository --persistence false` |
| GitNexus | 1.6.9 | `gitnexus analyze --index-only` |
| Graphify | 0.9.59 | `graphify extract --code-only --no-cluster` |
| Gortex | v0.64.3 | resident daemon, measured at its own log gates |

Corpora are APFS clones of snapshots pinned at each repository's HEAD. Each tool
gets its **own** copy, because these tools write index state into or beside the
tree. No working checkout and no live product index was read or written.

| repo | commit | files | symbols | edges |
|---|---|---:|---:|---:|
| DevPrism | `feaef2ffd27f` | 595 | 6,290 | 18,534 |
| DevCouncil | `1d680486869d` | 1,098 | 11,473 | 37,326 |
| GitPulse | `e9322f5361a7` | 1,999 | 27,918 | 91,292 |
| scholarlm | `5b5454c88cc1` | 4,335 | 41,995 | 146,115 |

File counts differ per tool — Gortex reindexed 4,425 files on scholarlm where
DevMap indexed 4,335 — so throughput is not comparable between tools.

## Cold index

Minimum seconds; `×N` is times DevMap's, from unrounded values.

| repo | DevMap | CodeGraph | CBM | GitNexus | Graphify |
|---|---:|---:|---:|---:|---:|
| DevPrism | **0.757s** | 2.085s ×2.8 | 5.703s ×7.5 | 15.928s ×21 | 5.081s ×6.7 |
| DevCouncil | **2.012s** | 3.294s ×1.6 | 9.357s ×4.7 | 33.689s ×17 | 14.989s ×7.4 |
| GitPulse | **2.820s** | 6.372s ×2.3 | 5.690s ×2.0 | 43.654s ×15 | 12.044s ×4.3 |
| scholarlm | **4.611s** | 9.036s ×2.0 | 7.367s ×1.6 | 42.948s ×9.3 | 26.788s ×5.8 |

DevMap had the lowest cold time on all four corpora, by between 1.6× and 21×.

CBM barely responds to corpus size — 5.7s on 595 files and 7.4s on 4,335 — so most
of its cold number is fixed process cost, not indexing. Its margin against DevMap
narrows on the largest corpus purely because DevMap's own cost grows with the tree.

## Unchanged refresh (`warm`)

| repo | DevMap | CodeGraph | CBM | GitNexus | Graphify |
|---|---:|---:|---:|---:|---:|
| DevPrism | **0.053s** | 0.194s ×3.7 | 5.657s ×107 | 0.528s ×9.9 | 2.105s ×40 |
| DevCouncil | **0.089s** | 0.235s ×2.6 | 5.726s ×64 | 0.699s ×7.8 | 5.080s ×57 |
| GitPulse | **0.102s** | 0.222s ×2.2 | 5.692s ×56 | 0.765s ×7.5 | 6.890s ×67 |
| scholarlm | **0.141s** | 0.425s ×3.0 | 5.863s ×42 | 1.207s ×8.6 | 14.044s ×100 |

This is the watcher's common tick and DevMap's largest margin. It is the one stage
where DevMap stays close to flat as the corpus grows (0.053s → 0.141s for 7× the
files) while Graphify grows nearly 7×.

## One file changed (`touch`)

| repo | DevMap | CodeGraph | CBM | GitNexus | Graphify |
|---|---:|---:|---:|---:|---:|
| DevPrism | 0.389s | **0.360s ×0.92** | 6.002s ×15 | 18.600s ×48 | 2.136s ×5.5 |
| DevCouncil | 0.816s | **0.512s ×0.63** | 8.894s ×11 | 31.841s ×39 | 4.883s ×6.0 |
| GitPulse | 1.519s | **0.421s ×0.28** | 5.674s ×3.7 | 40.327s ×27 | 6.696s ×4.4 |
| scholarlm | 2.449s | **0.867s ×0.35** | 7.292s ×3.0 | 77.161s ×32 | 14.275s ×5.8 |

**This is still the one stage DevMap loses, on every corpus.** DevMap is 1.08×
CodeGraph's time on DevPrism, 1.59× on DevCouncil, **3.61× on GitPulse** and 2.82×
on scholarlm.

Note that, unlike in the prior report, the gap **no longer widens monotonically
with corpus size** — it peaks at GitPulse and then narrows on the corpus twice its
size. That is v0.2.2's doing: the A/B below shows the release took 27% off
scholarlm's `touch` while leaving GitPulse's unchanged.

The cause is known, measured and unchanged in kind: roughly 59% of an incremental
build is whole-tree work that does not shrink with the change set, because
resolution deliberately covers the whole tree so liveness and community detection
mean the same thing on both paths. v0.2.2 has made this materially cheaper on the
largest corpus (see the A/B below: scholarlm `touch` is 1.37× faster than v0.2.1),
but it has not changed the shape of the curve, and CodeGraph still wins the stage.

GitNexus is slowest here because the run used its full `analyze` path rather than
an incremental one; that is this harness's choice, not a GitNexus limit.

## Gortex (resident daemon — not comparable to the tables above)

Gortex holds a daemon rather than exiting, so a CLI wall-clock wrapper would
measure the wrong thing. These are its own log gates, **n=1**, with a
`files_reindexed > 0` assertion so a daemon that warmed over an empty graph cannot
be recorded as a measurement.

| repo | query-ready | enrichment complete | files reindexed | database | load |
|---|---:|---:|---:|---:|---:|
| DevPrism | 7.36s | 13.98s | 758 | 186 MB | 7.3 |
| DevCouncil | 16.51s | 36.88s | 1,366 | 398 MB | 7.9 |
| GitPulse | 29.25s | 82.70s | 2,068 | 700 MB | 6.6 |
| scholarlm | 107.95s | 269.37s | 4,425 | 2,333 MB | 5.4 |

`query-ready` is the earliest point queries are served; enrichment continues behind
it. Against a resident daemon, a standalone CLI pays process startup on every call
that Gortex pays once — the reason these sit in their own table.

The prior report's scholarlm figure (122.0s) was taken at a 1-minute load average of
16.5 and flagged there as an upper bound. This run measured 107.95s at load 5.4,
which is consistent with that caveat having been correct.

## Definitions, callers, and freshness

Everything in this section runs on the **DevCouncil corpus only**, because that
is where the caller pairs were source-inspected. All five pairs were re-verified
against *this* commit before the run — call sites at
`test_scholarlm_findings.rs:781-794`, `model.rs:1634-1644`,
`compact_test.go:547,550`, `repomap.go:327`, `map_bench.py:1074`. A ground truth
carried forward unchecked is not a ground truth.

Five repetitions per symbol per tool; three symbols, one each in Rust, Go and
Python. "Exact definitions" is 15 = 3 symbols × 5 repetitions, and requires the
tool to return the symbol at its exact source path.

| Tool | Exact definitions | Caller pairs | Edits visible | Deleted probe absent | Revert-to-original |
|---|---:|---:|---:|---:|---|
| **DevMap 0.2.2** | 15/15 | **5/5** | 3/3 | 2/2 | clean |
| CodeGraph | 15/15 | 3/5 | 3/3 | 2/2 | clean |
| CBM | 15/15 | **5/5** | 3/3 | 2/2 | clean |
| GitNexus | 15/15 | 3/5 | 3/3 | 2/2 | **stale**, forced rebuild clears it |
| Graphify | 15/15 | 3/5 | 3/3 | 2/2 | clean |
| Gortex | 15/15 | 4/5 | 3/3 | 2/2 | *not measured* |

Every tool found every definition, in every repetition, and every tool made all
three appended functions visible after its own refresh. **The separation is
entirely in the caller column**, and it reproduces the build-48cd3c7 result
tool-for-tool: DevMap 5/5, CBM 5/5, Gortex 4/5, CodeGraph / GitNexus / Graphify
3/5.

Missing pairs in the default responses:

- DevMap 0.2.2: none in this five-pair sample.
- CodeGraph: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes`
- CBM: none in this five-pair sample.
- GitNexus: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes`
- Graphify: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes`
- Gortex: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`

Three tools miss the same two Rust pairs — both are `#[test]` functions calling
`db_size_gate_bytes`. Gortex misses one of them by default and returns **5/5**
when asked through `get_callers` with `min_tier: "text_matched"` and
`exclude_tests: false`. That supplemental request does not replace its default
score: name-only edges are inferred, not resolved.

Only these five pairs are scored. Additional returned edges are left unjudged,
so this measures **recall on a five-pair sample**, not precision and not general
graph accuracy.

## Index staleness: the case that separates the tools

The deleted-probe column above is the easy version of the question — remove one
of three appended functions and the file is left holding content the tool has
never seen, which any change-detector notices. Every tool passed it.

The harder case is reverting a file to bytes the tool **already indexed**. A
tool keyed on content hash or mtime can conclude "identical to the cold
generation, nothing to do" and skip re-analysis, keeping the symbols it learned
from the intermediate generation. [`staleness.py`](staleness.py) runs exactly
that: append a probe, refresh, restore the original bytes, refresh, query.

| Tool | Probe indexed | Present after 1 refresh | After 2 refreshes | Verdict |
|---|---|---|---|---|
| **DevMap 0.2.2** | yes | no | no | clean |
| CodeGraph | yes | no | no | clean |
| CBM | yes | no | no | clean |
| Graphify | yes | no | no | clean |
| GitNexus | yes | **yes** | **yes** | **stale** — a forced rebuild clears it |

**GitNexus serves a symbol that no longer exists in the source**, through two
ordinary refreshes. This reproduces the build-48cd3c7 finding and adds its
mechanism: the first probe design in this run did *not* catch it, because it
left the file with novel content. The difference between the two probes is the
whole finding, and it is why the weaker probe is reported beside the stronger
one rather than instead of it.

## Query latency

Medians over five repetitions. Search is each tool's native definition lookup;
callers is its native inbound traversal. Gortex includes client-to-daemon
transport; every other tool starts a standalone process, so its number carries
process startup that a persistent server would pay once.

| Tool | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |
|---|---:|---:|---:|---:|---:|---:|
| **DevMap 0.2.2** | **9.7 ms** | **9.7 ms** | **9.7 ms** | **27.8 ms** | **28.4 ms** | **27.7 ms** |
| CodeGraph | 105.8 ms | 102.3 ms | 101.0 ms | 101.6 ms | 105.0 ms | 105.2 ms |
| CBM | 3937.6 ms | 3948.5 ms | 3948.3 ms | 3931.4 ms | 3937.7 ms | 3935.2 ms |
| GitNexus | 808.0 ms | 802.0 ms | 791.5 ms | 799.9 ms | 796.7 ms | 800.1 ms |
| Graphify | 571.7 ms | 557.0 ms | 567.2 ms | 267.2 ms | 261.4 ms | 254.8 ms |
| Gortex | 103.5 ms | 100.6 ms | 91.6 ms | 92.7 ms | 81.6 ms | 75.3 ms |
| ripgrep (text) | 45.8 ms | 43.8 ms | 44.8 ms | — | — | — |

| Compared with | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |
|---|---:|---:|---:|---:|---:|---:|
| CodeGraph | 10.9× faster | 10.5× faster | 10.4× faster | 3.7× faster | 3.7× faster | 3.8× faster |
| CBM | 406.3× faster | 407.1× faster | 408.4× faster | 141.5× faster | 138.6× faster | 141.8× faster |
| GitNexus | 83.4× faster | 82.7× faster | 81.9× faster | 28.8× faster | 28.0× faster | 28.8× faster |
| Graphify | 59.0× faster | 57.4× faster | 58.7× faster | 9.6× faster | 9.2× faster | 9.2× faster |
| Gortex | 10.7× faster | 10.4× faster | 9.5× faster | 3.3× faster | 2.9× faster | 2.7× faster |

DevMap had the lowest median in all six query cells. Two readings that are
**not** supported by this table:

- **CBM's ~3.9 s is not its engine latency.** Each call opens a fresh process.
  Its persistent MCP path was not measured, here or in any prior report.
- **The ripgrep row is not a competitor.** Literal text occurrence is a different
  operation from symbol lookup against a graph, and it establishes no definition,
  caller, or impact. It is a floor for "what does touching every byte cost".

Outputs also differ in scope: DevMap `explore` returns a composed neighborhood,
Graphify `explain` includes connections, GitNexus `context --content` includes
source, while CodeGraph `callers` and Gortex depth-one traversal are narrower.
**A missing caller is a correctness miss, not fast traversal.**

## Coverage inventories and warnings

Counts preserve each tool's own native surface and stage. They are not accuracy
scores, and the tools do not count the same things.

| Native surface | Reported file measure | Symbols/nodes | Edges |
|---|---|---:|---:|
| DevMap cold | 1,098 indexed | 11,473 | 37,326 |
| CodeGraph cold | 843 indexed | 16,529 | 60,115 |
| Graphify cold | 1,200 files | 14,204 | 38,495 |
| GitNexus cold | 1 large file skipped | 21,325 | 56,124 |
| CBM cold full | no comparable total | 29,933 | 126,324 |
| Gortex daemon warmup | 1,366 reindexed | — | — |

DevMap reports **182,181 unresolved call sites, of which 75,964 are classified
as explained, leaving 106,217 unexplained** — the single largest coverage
warning any tool in this run volunteers about itself. It also reports 0 files
refused at discovery and 0 extraction failures, 480 communities and 54 dead-code
candidates.

GitNexus skipped one file above its 512 KB cap:
`benchmarks/results/competition/20260912-expanded/gortex-source-tree.json` — a
prior benchmark's own artifact.

**The corpus contaminates its own probes, and increasingly so.** DevCouncil
commits its benchmark outputs, so the tree now contains hundreds of `.stdout`
files and several `write_report.py` scripts that contain the literal strings
`db_size_gate_bytes`, `loadBounded` and `describe_corpus`. CodeGraph's search
response for the Rust probe returns, after the real definition, two `TARGETS`
variables from *previous runs' report generators*. This does not affect the
exact-path scoring, which requires the symbol at its source path, but it inflates
the ripgrep baseline and adds noise to every fuzzy search — and it grows with
each committed report, including this one.

## Peak memory, cold index (sampled process tree, MB)

| repo | DevMap | CodeGraph | GitNexus | Graphify |
|---|---:|---:|---:|---:|
| DevPrism | **414** | 940 | 2,931 | 1,100 |
| DevCouncil | **678** | 2,430 | 3,355 | 3,417 |
| GitPulse | **1,116** | 2,364 | 4,768 | 1,406 |
| scholarlm | 1,725 | 2,884 | 4,814 | **1,458** |

**This contradicts the prior report's claim that DevMap used the least memory on
every corpus.** On scholarlm, Graphify's 1,458 MB is below DevMap's 1,725 MB.
Graphify's sampled peak barely moves between GitPulse and scholarlm (1,406 →
1,458 MB) while DevMap's grows with the corpus, so on a large enough tree Graphify
wins this column. DevMap still leads on the other three.

**CBM is deliberately absent from the table.** The prior run measured its sampled
peak varying −75% to −96% between two passes, because its workers are too
short-lived for the 50 ms sampler to catch reliably. This run took one pass and so
can neither confirm nor refute that; what it caught is recorded in
`comparison.json` as `cbm_cold_peak_rss_mb` (DevPrism 170, DevCouncil 1,589,
GitPulse 1,777, scholarlm 3,930 MB) and should not be treated as a stable figure.

## Index size on disk (MB)

| repo | DevMap | CodeGraph | Graphify | GitNexus | Gortex |
|---|---:|---:|---:|---:|---:|
| DevPrism | 83 | 49 | **18** | 254 | 188 |
| DevCouncil | 144 | 73 | **49** | 470 | 398 |
| GitPulse | 272 | 168 | **85** | 648 | 701 |
| scholarlm | 464 | 295 | **167** | 743 | 2,334 |

DevMap's store is 1.6–2.0× CodeGraph's and 2.8–4.7× Graphify's, while sitting at
31–62% of GitNexus's. It is smaller in absolute terms than the prior report's
figures on every corpus, and the A/B below confirms that shrink is the release's
doing rather than the corpora's. CBM stores nothing here — it ran with
`--persistence false`.

## v0.2.1 → v0.2.2: the controlled A/B

The matrix above cannot answer "did v0.2.2 get faster". Comparing its DevMap column
against the prior report's would be a *sequential* before/after taken a day apart on
a shared interactive machine — which `benchmarks/README.md` documents as unreliable:
three consecutive cold runs of one unchanged binary measured 4.35s, 2.96s and 5.20s
on this host.

So both binaries were run **alternating A,B,A,B against the same corpora in the same
session**, three rounds, each round rebuilding a scratch store from cold, with the
leading arm swapped every round so neither binary owns the cold-cache position.
v0.2.1 was built from the `v0.2.1` tag (`bde133a97077`); v0.2.2 is the `v0.2.2` tag
(`1d680486869d`). The two differ observably: store schema 20 against 22.

### The graphs are identical

| repo | files / symbols / edges / dead / communities |
|---|---|
| DevPrism | identical |
| DevCouncil | identical |
| GitPulse | identical |
| scholarlm | identical |

Across all four corpora v0.2.2 extracted exactly what v0.2.1 extracted. **The
speedups below are not v0.2.2 doing less work.**

### Timings (minimum of 3 interleaved rounds)

| repo | stage | v0.2.1 | v0.2.2 | change | |
|---|---|---:|---:|---:|---|
| DevPrism | cold | 0.866s | 0.728s | **−16.0%** | 1.19× faster |
| DevPrism | warm | 0.041s | 0.046s | +12.5% | real, see below |
| DevPrism | touch | 0.380s | 0.367s | −3.4% | *< spread* |
| DevCouncil | cold | 2.187s | 1.912s | **−12.6%** | 1.14× faster |
| DevCouncil | warm | 0.076s | 0.074s | −2.5% | *< spread* |
| DevCouncil | touch | 0.818s | 0.809s | −1.1% | *< spread* |
| GitPulse | cold | 3.345s | 2.827s | **−15.5%** | 1.18× faster |
| GitPulse | warm | 0.088s | 0.089s | +0.6% | *< spread* |
| GitPulse | touch | 1.502s | 1.491s | −0.8% | *< spread* |
| scholarlm | cold | 6.307s | 4.515s | **−28.4%** | 1.40× faster |
| scholarlm | warm | 0.117s | 0.125s | +6.1% | *< spread* |
| scholarlm | touch | 3.364s | 2.455s | **−27.0%** | 1.37× faster |

*< spread* marks a difference smaller than the larger arm's own min-to-max range.
Those rows are not results; they are noise, and are labelled rather than rounded
into the win column.

### Store size

| repo | v0.2.1 | v0.2.2 | change |
|---|---:|---:|---:|
| DevPrism | 83 MB | 67 MB | **−18.8%** |
| DevCouncil | 156 MB | 131 MB | **−16.3%** |
| GitPulse | 310 MB | 253 MB | **−18.4%** |
| scholarlm | 517 MB | 430 MB | **−16.8%** |

### Query latency and accuracy did not change

DevMap's query medians above (9.7 ms search, ~28 ms callers) sit at roughly a
third of the build-48cd3c7 report's (30.0 ms and 51.2 ms). It would be easy, and
wrong, to bank that as a v0.2.2 win. [`query_ab.py`](query_ab.py) put both
binaries on the same corpus in the same session, five alternating rounds, each
from its own freshly built store:

| Operation | Symbol | v0.2.1 | v0.2.2 | change |
|---|---|---:|---:|---:|
| search | `db_size_gate_bytes` | 10.3 ms | 10.5 ms | +2.0% *< spread* |
| search | `loadBounded` | 10.0 ms | 10.1 ms | +1.2% *< spread* |
| search | `describe_corpus` | 10.3 ms | 9.9 ms | −3.4% *< spread* |
| callers | `db_size_gate_bytes` | 28.7 ms | 29.5 ms | +2.7% *< spread* |
| callers | `loadBounded` | 28.5 ms | 28.5 ms | +0.1% *< spread* |
| callers | `describe_corpus` | 28.6 ms | 29.0 ms | +1.4% *< spread* |

**Every cell is within spread: query latency is unchanged between the releases.**
v0.2.1 measures ~10 ms here too. The gap against the earlier report is campaign
conditions — its query samples ran interleaved with competitor processes under
heavier desktop load — not a release improvement. That report predicted exactly
this failure mode in its own words: its campaign and its own alternating control
disagreed by more for one build across sessions than two builds disagreed within
one.

Correctness is also unchanged: both arms returned **15/15 exact definitions and
5/5 caller pairs**. So v0.2.2's indexing gains cost nothing in graph quality on
this corpus — and bought nothing in query speed.

### What v0.2.2 actually bought, and what it cost

**Bought:** cold indexing 1.14–1.40× faster on every corpus; the incremental edit
1.37× faster on the largest corpus; the store 16–19% smaller everywhere — for a
byte-identical graph, unchanged query latency and unchanged caller accuracy.

**Cost:** on DevPrism, the unchanged-refresh got measurably *slower* — v0.2.1's
three samples were 41/43/44 ms and v0.2.2's were 46/50/51 ms. The ranges do not
overlap, so unlike the other warm rows this one is a real regression, not noise.
It is ~5 ms on the smallest corpus and it does not reproduce on the other three
(all three overlap and are non-results), but it is the one place this release moved
backwards and it should not be rounded away.

## Positives, negatives, and comparison limits

Observations about this workload, these interfaces, and selected source cases.
A smaller store or a lower latency is not equivalent graph quality.

| Tool | Positives observed | Negatives and limits |
|---|---|---|
| **DevMap 0.2.2** | Lowest cold and unchanged-refresh time on all four corpora; lowest median in all 6 query cells; 5/5 caller pairs; 3/3 edits visible; clean on both staleness probes. v0.2.2 cuts cold 1.14–1.40× and the store 16–19% against v0.2.1 for an identical graph. | **Loses the one-file edit on every corpus**, 1.08–3.61× CodeGraph's time. **106,217 unexplained attribution sites.** No longer lowest memory on the largest corpus (1,725 MB against Graphify's 1,458). Unchanged refresh regressed ~12% on the smallest corpus. Five caller pairs are not accuracy coverage. |
| CodeGraph | Fastest one-file edit of any tool, on all four corpora — 0.28× DevMap's on GitPulse. Second-lowest query latency among standalone CLIs. Store 1.6–2.0× smaller than DevMap's. 3/3 edits visible; clean on both staleness probes. | 3/5 caller pairs, missing both Rust `#[test]` callers. Cold 1.6–2.8× DevMap's; queries ~10× DevMap's. Cold RSS 2,430 MB on DevCouncil. Indexed 843 files where DevMap indexed 1,098 — different scope, not a like-for-like count. |
| CBM | **5/5 caller pairs**, matching DevMap. Highest node and edge counts of any tool. Near-flat cold time across a 7× corpus range. 3/3 edits visible; clean on both staleness probes. | Slowest queries by two orders of magnitude (~3.9 s), though that includes fresh process startup and **does not qualify its persistent MCP path**. Unchanged refresh 5.7 s — 42–107× DevMap's. Ran with `--persistence false`, so it leaves no store to measure. Sampled memory too unstable to report. |
| Graphify | Smallest store on every corpus, 2.8–4.7× smaller than DevMap's. **Lowest sampled memory on scholarlm (1,458 MB), beating DevMap.** All definitions found; 3/3 edits visible; clean on both staleness probes. AST-only extraction, no hosted inference. | 3/5 caller pairs. Slowest unchanged refresh of the CLI tools relative to corpus size — 14.0 s on scholarlm, 100× DevMap's. Search ~570 ms. Clustering and LLM enrichment were disabled, so full-feature quality is unmeasured. |
| GitNexus | All definitions found; 3/3 edits visible. Context responses carry source and relationships. Unchanged refresh is 20–60× cheaper than its own cold path. | **Serves a deleted symbol through two ordinary refreshes** when a file is reverted to previously-indexed content; only a forced rebuild clears it. 3/5 caller pairs. Slowest cold (9.3–21× DevMap's) and slowest edit (27–48×). Largest store on the two smaller corpora, behind Gortex on the two larger; highest cold RSS throughout. |
| Gortex | 4/5 caller pairs by default and **5/5** with name-only inclusion. Query latency competitive with CodeGraph while serving from a warm daemon. Exposes separate query-ready and enrichment gates. | Largest store by far on scholarlm (2,334 MB). Daemon timings are not comparable to standalone CLIs, and are n=1. Name-only caller edges are inferred, not resolved. Staleness under revert was not measured for it. |
| ripgrep | An independent text-occurrence floor, needing no graph at all: ~45 ms. | Occurrences establish no definition, caller, confidence or impact. Not a competitor, and assigned no accuracy score. Inflated here by the corpus's own committed benchmark artifacts. |

## How much to trust this

- **These tools do not do the same work.** Timings describe each tool's own native
  command. Nothing here measures caller accuracy, query quality, or symbol
  resolution correctness, so no row is a quality ranking.
- **The machine was shared.** 1-minute load averages across the 180 matrix samples
  ranged 5.8–11.2 (per-sample `load_average` is in `measurements.jsonl`). Minimums
  are the defensible statistic. This is a tighter band than the prior run's 5–18.
- **Gortex is n=1** and measured at different gates than everything else.
- **One stage per tool per scenario.** No tool was tuned, and a faster mode may
  exist for any of them — GitNexus's `touch` in particular used its full `analyze`
  path.
- **The edit is a content change that may not parse.** The `touch` stage appends
  `# devmap-bench edit` to a `.rs`/`.tsx`/`.go` file, where `#` does not begin a
  comment. Every tool sees the identical edit and it is the same edit the prior
  reports made, so it is comparable — but it is not a syntactically valid change.
- **Cross-report competitor comparison is not controlled.** See the provenance
  section. The two A/Bs are the only version claims in this document that are —
  and the query A/B exists precisely because the uncontrolled reading of the same
  numbers said "3× faster" and was wrong.
- **Snapshots exclude uncommitted work.**
- **The correctness campaign is one corpus, three symbols, five caller pairs.**
  It measures recall on a hand-inspected sample in Rust, Go and Python. It is not
  precision, not a population estimate, and not general graph accuracy. Additional
  returned edges are unjudged.
- **Persistent-MCP latency is unmeasured** for every tool that offers one, so no
  query row describes a warm server except Gortex's.
- **The corpus contains this benchmark's own committed outputs**, which mention
  all three probe symbols. Exact-path scoring is unaffected; fuzzy search and the
  ripgrep baseline are not.
- **The ripgrep baseline ran with `RIPGREP_CONFIG_PATH=` cleared**, unlike the
  prior report, which inherited this machine's `~/.ripgreprc`. Stock behaviour is
  the reproducible choice; it makes that one row not directly comparable.
- **Gortex's staleness under revert was not measured**, so its row in the
  correctness table is blank rather than a pass.

## Reproducing

```bash
python3 install_tools.py             # re-fetch pinned tools, verify every sha256
python3 make_plan.py                 # pin binaries, snapshots, edit targets
python3 run.py all full 3            # the five-CLI matrix, interleaved
python3 gortex_env.py                # private socket / XDG roots
python3 gortex_cold.py all 1         # daemon gates, with the indexed assertion
python3 analyze.py && python3 finalize.py
python3 ab_control.py 3              # v0.2.1 vs v0.2.2, interleaved (digest-checked)

python3 semantics.py all             # definitions, callers, edits, negatives, ripgrep
python3 gortex_semantics.py          # the same campaign against Gortex's daemon
python3 staleness.py all             # the revert-to-original staleness probe
python3 query_ab.py 5                # v0.2.1 vs v0.2.2 query latency + accuracy
python3 audit_semantics.py           # score the campaign -> semantics-audit.json

python3 render_tables.py             # regenerate the timing/memory/store tables
python3 render_semantics.py          # regenerate the correctness/query tables
python3 verify_report.py             # assert REPORT.md still matches the raw data
python3 verify_docs.py               # assert README/guide/site copy match it too
```

`verify_docs.py` is the guard against the failure this repository's own website
copy warns about: it checks that every headline figure quoted in `README.md`,
the [DevMap guide], the [comparison page] and the [site copy] is one this run actually
measured, that none still quote the superseded figures, and that **every one of
them states CodeGraph's edit win**. A page that published only the wins would
fail it.

[DevMap guide]: ../../../../docs/devmap/README.md
[comparison page]: ../../../../docs/devmap/comparison.md
[site copy]: ../../../../DevCouncil-website-benchmark.md

`ab_control.py` refuses to run unless both arms match the sha256 recorded in
[`ab-binaries.json`](ab-binaries.json). The v0.2.1 arm was built for this run from
the `v0.2.1` tag into scratch — that path does not survive the session, and the
file records the command to rebuild it.
