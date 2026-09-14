# DevMap competitor benchmarks

- [**v0.2.2 full comparison** — six graph tools across four repositories: indexing, callers, queries, staleness, plus two controlled v0.2.1/v0.2.2 A/Bs](20260914-v0.2.2/REPORT.md)
- [Four-repository comparison — DevMap against five graph tools on DevPrism, DevCouncil, GitPulse and scholarlm](20260913-multirepo/REPORT.md)
- [DevMap build 48cd3c7 rerun — six graph tools, ripgrep, and alternating build control](20260913-48cd3c7/REPORT.md)
- [Prior v0.2.1 run against build 506a617](20260913-v0.2.1/REPORT.md)
- [Prior v0.2.0 expanded comparison — Graphify, Gortex, GitNexus, CodeGraph, codebase-memory-mcp, and ripgrep](20260912-expanded/REPORT.md)
- [Initial DevMap/GitNexus comparison and Rust verification](20260912-52d63a1a/REPORT.md)

The build-`48cd3c7` report is the latest source for per-tool **positives,
negatives, and comparisons**: indexing and query latency with explicit
“× faster/slower” factors, memory and storage, caller checks, update/deletion
behavior, warnings and recovery, methodology, unverified scenarios, and
inferred DevMap improvement priorities. Its raw measurements and audit remain
linked beside the conclusions.

It exists because the installed DevMap executable advanced 49 commits past the
build the v0.2.1 report measured, so that report stopped describing the
shipping binary. Both are version 0.2.1 and are distinguished by build id.

The **v0.2.2 run** is the first to carry both halves in one report: the
four-repository indexing/memory/store matrix *and* the correctness-and-query
campaign (definitions, callers, edit visibility, staleness, query latency,
coverage inventories) that until now lived only in the single-corpus reports.
It also adds what no earlier report has — **two interleaved A/Bs between v0.2.1
and v0.2.2 on the same corpora in the same session**.

Its headlines: cold indexing 1.14–1.40× faster, scholarlm's one-file edit 1.37×
faster, the store 16–19% smaller, for a graph identical to v0.2.1's on all four
corpora — while **query latency and caller accuracy did not change at all**.
That last point is why the query A/B exists: read across reports, the same
numbers appear to show a 3× query speedup, and the controlled measurement shows
none. Against those gains sit one real regression (DevPrism's unchanged refresh,
+12%, ~5 ms) and two places DevMap no longer leads: Graphify uses less memory on
scholarlm, and CodeGraph still wins the one-file edit on every corpus.

On correctness it reproduces build-48cd3c7 tool-for-tool — DevMap 5/5 caller
pairs, CBM 5/5, Gortex 4/5 (5/5 opt-in), CodeGraph/GitNexus/Graphify 3/5 — and
confirms GitNexus serving a deleted symbol through two ordinary refreshes, now
with the mechanism identified: it only happens when a file is reverted to
previously-indexed content.

Its competitor executables were deleted by a 2026-09-13 cleanup pass and
reinstalled from their pinned releases, each verified byte-for-byte against the
digest the build-48cd3c7 run recorded; its corpora, however, sit at newer HEADs,
so only its two A/B sections are controlled version comparisons.

The four-repository runs are the exception to the shared-corpus note below: it
measures DevPrism, DevCouncil, GitPulse and scholarlm at their own HEADs, to
test whether the ordering between tools survives a change of corpus size and
language mix. It covers cold, unchanged-refresh and one-file-edit latency,
memory and index size only — it has no caller-accuracy checks, no persistent-MCP
timings and no stale-index recovery, so it supplements the reports below rather
than superseding them. Its headline: DevMap led cold and unchanged refresh on
all four corpora, and lost the one-file edit to CodeGraph by a margin that grew
with corpus size, for the whole-tree-resolution reason its own
[map benchmark](../map/20260913T195023Z.md) measured the same day.

The remaining runs use the same frozen DevCouncil corpus at commit `ee07c183`. Their
samples were collected separately. The latest rerun has 261 competitive timing
samples plus 82 supplemental samples alternating the preserved `506a617` and
`48cd3c7` binaries. Keep each run's campaigns and the historical timings
separate. The initial run retains its own measurements and separate Rust
verification evidence. Read each report's interface, coverage, and
reproducibility limits before comparing results. The
[DevMap guide](../../../docs/devmap/README.md#benchmark-comparison) provides a
shorter summary without replacing the full report.
