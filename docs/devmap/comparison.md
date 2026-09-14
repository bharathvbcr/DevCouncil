# DevMap vs GitNexus, CodeGraph, Graphify, Gortex, and codebase-memory-mcp

**Short answer:** DevMap 0.2.2 was the fastest of six code-graph tools at cold
indexing and unchanged refresh on **all four repositories tested**, had the
lowest median in **all six symbol-query cells**, and found **all five**
source-inspected caller pairs — more than any tool except codebase-memory-mcp,
which matched it. CodeGraph beat it on one axis that matters, re-indexing a
single edited file, on every corpus. Graphify and CodeGraph ship smaller stores,
and Graphify used less memory than DevMap on the largest repository.

Every figure here is measured, from a reproducible run whose raw output is
committed in this repository. Read the
[full benchmark report](../../benchmarks/results/competition/20260914-v0.2.2/REPORT.md)
for the method, the failures, and the limits before quoting any of it.

## Indexing, across four repositories

Each tool indexed its own copy of four repositories — 595 to 4,335 files, mixing
Rust, Go, TypeScript, Python and Swift. Minimum of three interleaved repeats.
`×N` is the competitor's time as a multiple of DevMap's.

| Stage | DevMap 0.2.2 | Result |
|---|---|---|
| Cold index | 0.757 s – 4.611 s | **Fastest on all four**, by 1.6× to 21× |
| Unchanged refresh | 0.053 s – 0.141 s | **Fastest on all four**, by 2.2× to 107× |
| One file changed | 0.389 s – 2.449 s | **Slowest of the two leaders** — CodeGraph wins all four |

The unchanged-refresh margin is the widest and the most load-bearing: it is the
watcher's common tick, and DevMap stays near-flat as the corpus grows (0.053 s
at 595 files to 0.141 s at 4,335) while Graphify grows nearly sevenfold.

## Head-to-head on one corpus

The detailed table uses the 1,098-file DevCouncil corpus, where the correctness
and query campaign also ran. Cold/refresh/edit are minimums of three interleaved
repeats; queries are medians of five.

| Tool | Version | Cold index | Unchanged refresh | Single-file edit | Definition lookup | Caller lookup | Peak RSS | Index size | Caller pairs |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| **DevMap** | 0.2.2 | **2.012 s** | **0.089 s** | 0.816 s | **9.7 ms** | **27.7–28.4 ms** | **678 MiB** | 144 MiB | **5/5** |
| CodeGraph | 1.6.0 | 3.294 s | 0.235 s | **0.512 s** | 101–106 ms | 102–105 ms | 2430 MiB | 73 MiB | 3/5 |
| codebase-memory-mcp | 0.10.8 | 9.357 s | 5.726 s | 8.894 s | ~3.94 s | ~3.93 s | — | — | **5/5** |
| Graphify | 0.9.59 | 14.989 s | 5.080 s | 4.883 s | 557–572 ms | 255–267 ms | 3417 MiB | **49 MiB** | 3/5 |
| GitNexus | 1.6.9 | 33.689 s | 0.699 s | 31.841 s | 792–808 ms | 797–800 ms | 3355 MiB | 470 MiB | 3/5 |
| Gortex | 0.64.3 | 16.5 s to query-ready | — | — | 92–104 ms | 75–93 ms | — | 398 MiB | 4/5 |

Gortex runs a resident daemon, so its cold figure is a log gate rather than a
CLI wall time and is not comparable to the column. codebase-memory-mcp ran with
`--persistence false`, so it leaves no store; its sampled memory was too
unstable to report.

A ripgrep text baseline searched the same corpus in ~45 ms. That is a different
operation — literal text occurrences, not symbol resolution — included so the
graph tools can be read against something that builds no graph at all.

## Correctness, not just speed

Three symbols (one each in Rust, Go and Python), five repetitions, and five
direct caller pairs inspected in the source before the run.

| Tool | Exact definitions | Caller pairs | Edits visible | Stale after revert |
|---|---:|---:|---:|---|
| **DevMap 0.2.2** | 15/15 | **5/5** | 3/3 | clean |
| CodeGraph | 15/15 | 3/5 | 3/3 | clean |
| codebase-memory-mcp | 15/15 | **5/5** | 3/3 | clean |
| Graphify | 15/15 | 3/5 | 3/3 | clean |
| GitNexus | 15/15 | 3/5 | 3/3 | **serves a deleted symbol** |
| Gortex | 15/15 | 4/5 (5/5 opt-in) | 3/3 | not measured |

Every tool found every definition and saw every edit. **The separation is
entirely in caller recall**, and three tools miss the same two Rust `#[test]`
callers.

## Where each tool stands

### DevMap vs CodeGraph

The closest comparison, and the one that splits. DevMap indexed cold **1.64×
faster** and refreshed an unchanged tree **2.63× faster** on this corpus, and
led on all four repositories at both stages. It answered definition lookups
**~10× faster** and caller lookups **~3.7× faster**, on a quarter of the memory.

**CodeGraph wins the edit loop, on every corpus.** DevMap takes 1.08× its time
on the smallest repository and **3.61×** on GitPulse. The cause is known and
measured: roughly 59% of a DevMap incremental build is whole-tree work that does
not shrink with the change set, because resolution deliberately covers the whole
tree so liveness and community detection mean the same thing on both paths. If
your workload is dominated by tight edit-reindex cycles rather than cold starts
and queries, that difference is real. CodeGraph also keeps a smaller store
(73 MiB vs 144 MiB here; 1.6–2.0× smaller across the four).

On the five inspected caller pairs CodeGraph returned 3/5, missing both Rust
test callers.

### DevMap vs GitNexus

The widest gap in the set. DevMap indexed cold **16.7× faster**, edited **39×
faster**, refreshed **7.8× faster**, and queried **~82× faster** on definitions
and **~29× faster** on callers.

GitNexus also produced the run's one correctness failure. When a file is
reverted to content GitNexus has already indexed, it keeps answering with a
symbol that no longer exists in the source, through two ordinary refreshes; only
a forced rebuild clears it. A weaker probe — leaving the file with content the
tool has never seen — does not catch this, which is why both probes are in the
report. GitNexus context responses do include source text and relationships,
which the narrower tools do not.

### DevMap vs codebase-memory-mcp

codebase-memory-mcp matched DevMap at **5/5 caller pairs** — the only other tool
to do so — and reports the highest node and edge counts in the set. Its measured
latency is far higher across the board (9.4 s cold, 5.7 s refresh, ~3.9 s
queries), but these timings come from its standalone CLI, which opens a fresh
process per call. **They do not qualify its persistent MCP engine**, which this
benchmark did not measure.

### DevMap vs Graphify

Graphify ships the **smallest index in the set** — 49 MiB here, and 2.8–4.7×
smaller than DevMap's across the four repositories — and completed AST-only
extraction with no hosted inference. It also **used less memory than DevMap on
the largest repository** (1,458 MB against 1,725 MB): its sampled peak barely
moves with corpus size while DevMap's grows.

Against that, it indexed cold 7.5× slower here, and its unchanged refresh
degrades badly with scale — 14.0 s on the 4,335-file corpus, ~100× DevMap's.
Its clustering and LLM enrichment were disabled for this run, so its
full-feature behaviour is unmeasured.

### DevMap vs Gortex

Gortex runs a resident daemon, so its query latency (75–104 ms) reflects
client-to-daemon transport rather than process startup, and its cold figure is a
log gate — query-ready at 16.5 s on this corpus, enrichment complete at 36.9 s.
It returned 4/5 caller pairs by default and **5/5** when explicitly asked to
include name-only matches at a lower confidence tier. Those name-only edges are
inferred rather than resolved. It carries by far the largest store on the
biggest corpus (2,334 MB).

## What changed in v0.2.2

Measured by running both binaries alternately on the same corpora in the same
session — the only controlled way to compare releases on a shared machine.

| | v0.2.1 → v0.2.2 |
|---|---|
| Cold index | **1.14–1.40× faster** on all four repositories |
| One-file edit | **1.37× faster** on the largest corpus; unchanged elsewhere |
| Store on disk | **16–19% smaller** on all four |
| Extracted graph | **identical** on all four |
| Query latency | **unchanged** — every cell within measurement spread |
| Caller accuracy | **unchanged** — 5/5 in both |
| Unchanged refresh | **~12% slower** on the smallest corpus (a real regression, ~5 ms) |

The query row matters for anyone reading across reports: the raw numbers appear
to show a 3× query speedup against the previous report, and the controlled A/B
shows none. The difference was campaign conditions, not the release.

## What this benchmark does not establish

- **Four repositories, one machine.** Three-to-five repetitions, three symbols,
  five inspected caller pairs. No population-level accuracy claim follows.
- **Latency is not analysis.** Output scopes differ: DevMap `explore` returns a
  broader neighbourhood, GitNexus returns source, CodeGraph and Gortex return
  narrower traversals. Equal milliseconds are not equal answers.
- **Caller recall on five pairs is not graph accuracy.** Additional returned
  edges are unjudged, so this is not a precision measurement.
- **Persistent MCP latency is unmeasured** for every tool except Gortex.
- **No coding-agent outcome** — task success, token savings, and answer
  usefulness were not evaluated.
- **DevMap reports 106,217 unexplained call-attribution sites** on this corpus,
  the largest coverage warning any tool here volunteers about itself.

## Reproducing it

The corpora are pinned at explicit commits, each tool gets its own clone, and
the competitor binaries are verified byte-for-byte against recorded SHA-256
digests before use. The scripts, every command's stdout/stderr, and the audit of
each semantic check are committed next to the report:

- [Full report](../../benchmarks/results/competition/20260914-v0.2.2/REPORT.md)
- [All runs, including prior versions](../../benchmarks/results/competition/README.md)
- [Benchmark hardening audit](BENCHMARK_HARDENING_AUDIT.md)

Earlier single-corpus reports remain published with their own measurements:
[build 48cd3c7](../../benchmarks/results/competition/20260913-48cd3c7/REPORT.md)
and [the four-repository v0.2.1 run](../../benchmarks/results/competition/20260913-multirepo/REPORT.md).
