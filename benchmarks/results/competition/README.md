# DevMap competitor benchmarks

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

The four-repository run is the exception to the shared-corpus note below: it
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
