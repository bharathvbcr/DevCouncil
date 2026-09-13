# DevMap vs GitNexus, CodeGraph, Graphify, Gortex, and codebase-memory-mcp

**Short answer:** on a frozen 1,186-file / 46.9 MB repository, DevMap 0.2.1 had
the fastest cold index (2.03 s), the fastest unchanged refresh (107 ms), the
fastest symbol queries of all six tools (30–33 ms definition lookups), the
lowest peak memory (610 MiB), and found all five source-inspected caller pairs.
CodeGraph beat it on one axis that matters — re-indexing a single edited file,
417 ms against DevMap's 958 ms — and three tools ship smaller indexes.

Every figure here is a measured median from a reproducible run whose raw output
is committed in this repository. Read the
[full benchmark report](../../benchmarks/results/competition/20260913-48cd3c7/REPORT.md)
for the method, the failures, and the limits before quoting any of it.

## Comparison table

All six tools indexed the identical corpus on one Apple M5 Pro (18 cores,
64 GiB). 261 timed samples: three cold builds, five unchanged refreshes, three
single-file edits, five repetitions per query cell.

| Tool | Version | Cold index | Unchanged refresh | Single-file edit | Definition lookup | Caller lookup | Peak RSS | Index size | Caller pairs |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| **DevMap** | 0.2.1 | **2.031 s** | **107.1 ms** | 957.8 ms | **30–33 ms** | **48–51 ms** | **610 MiB** | 139 MiB | **5/5** |
| CodeGraph | 1.6.0 | 2.912 s | 241.9 ms | **417.2 ms** | 144–166 ms | 151–167 ms | 2438 MiB | 63 MiB | 3/5 |
| codebase-memory-mcp | 0.10.8 | 8.019 s | 6.160 s | 9.347 s | 4.39–4.46 s | 4.35–4.53 s | 1208 MiB | 67 MiB | **5/5** |
| Graphify | 0.9.59 | 16.456 s | 4.327 s | 3.945 s | 567–615 ms | 271–292 ms | 3293 MiB | **42 MiB** | 3/5 |
| GitNexus | 1.6.9 | 34.102 s | 567.5 ms | 33.417 s | 980 ms–1.02 s | 964 ms–1.04 s | 3095 MiB | 265 MiB | 3/5 |
| Gortex | 0.64.3 | 38.835 s | 967.5 ms | 5.754 s | 92–115 ms | 85–106 ms | 2206 MiB | 344 MiB | 4/5 |

A ripgrep text baseline searched the same corpus in 35.7–36.4 ms. That is a
different operation — literal text occurrences, not symbol resolution — and is
included so the graph tools can be read against something that builds no graph
at all.

## Where each tool stands

### DevMap vs CodeGraph

The closest comparison. CodeGraph indexed cold in 2.912 s against DevMap's
2.031 s (**1.43× faster for DevMap**) and refreshed an unchanged tree in
241.9 ms against 107.1 ms (**2.26× faster for DevMap**). DevMap answered symbol
queries **2.9–5.2× faster**, and used 610 MiB peak against CodeGraph's
2438 MiB.

**CodeGraph wins the edit loop.** It re-indexed one changed file in 417.2 ms
against DevMap's 957.8 ms — 2.3× faster, and the gap widened from the previous
measured run. It also keeps a smaller store (63 MiB vs 139 MiB). If your
workload is dominated by tight edit-reindex cycles rather than cold starts and
queries, that difference is real.

On the five source-inspected caller pairs, CodeGraph returned 3/5, missing both
Rust test callers.

### DevMap vs GitNexus

The widest gap in the set. DevMap indexed cold **16.8× faster** (2.031 s vs
34.102 s), edited **34.9× faster** (957.8 ms vs 33.417 s), and queried
**19–33× faster** across the six cells (31–33× on definition lookups,
19–21× on caller lookups). GitNexus's unchanged refresh is respectable at
567.5 ms.

GitNexus also produced the run's one retained correctness failure: after a
symbol was deleted and the corpus restored, it kept answering with the deleted
symbol through two ordinary refreshes. Only a forced rebuild cleared it.
GitNexus context responses do include source text and relationships, which the
narrower tools do not.

### DevMap vs codebase-memory-mcp

codebase-memory-mcp matched DevMap at **5/5 caller pairs** — the only other
tool to do so — and ships a smaller 67 MiB store. Its measured latency is far
higher across the board (8.019 s cold, 6.160 s refresh, ~4.4 s queries), but
these timings come from its standalone CLI, which opens a fresh process per
call. **They do not qualify its persistent MCP engine**, which this benchmark
did not measure.

### DevMap vs Graphify

Graphify ships the **smallest index in the set at 42 MiB** and completed
AST-only extraction with no hosted inference. It indexed cold in 16.456 s
(DevMap 8.1× faster) and used the highest peak memory measured, 3293 MiB. Its
documentation, clustering, and LLM enrichment features were excluded from this
run, so its full-feature behaviour is unmeasured.

### DevMap vs Gortex

Gortex runs a resident daemon, so its query latency (92–115 ms) reflects
client-to-daemon transport rather than process startup, and its 38.835 s cold
figure includes wrapper and enrichment work — it reached a query-ready gate at
14.9 s. It returned 4/5 caller pairs by default and 5/5 when explicitly asked
to include name-only matches at a lower confidence tier.

## What this benchmark does not establish

- **One corpus, one machine.** Three-to-five repetitions, three symbols, five
  inspected caller pairs. No population-level accuracy claim follows.
- **Latency is not analysis.** Output scopes differ: DevMap `explore` returns a
  broader neighbourhood, GitNexus returns source, CodeGraph and Gortex return
  narrower traversals. Equal milliseconds are not equal answers.
- **Persistent MCP latency is unmeasured** for every tool except Gortex.
- **No coding-agent outcome** — task success, token savings, and answer
  usefulness were not evaluated here.
- Missing callers are correctness misses in this sample, not evidence of fast
  traversal.

## Reproducing it

The corpus is pinned at a single commit and hash-verified in six fresh clones
before and after each run. The scripts, the pinned competitor binary hashes,
every command's stdout/stderr, and the audit of each semantic check are
committed next to the report:

- [Full report](../../benchmarks/results/competition/20260913-48cd3c7/REPORT.md)
- [Method and protocol](../../benchmarks/results/competition/20260913-48cd3c7/PLAN.md)
- [Native warnings and limits](../../benchmarks/results/competition/20260913-48cd3c7/observations.md)
- [All runs, including prior versions](../../benchmarks/results/competition/README.md)

A supplemental 82-sample control alternated two DevMap builds 49 commits apart
on the same corpus. They produced a byte-identical graph — 857 files, 10,193
symbols, 33,789 edges — which is a useful reminder that this corpus does not
exercise every code path a release touches.
