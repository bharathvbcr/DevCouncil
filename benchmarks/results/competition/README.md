# DevMap competitor benchmarks

- [DevMap v0.2.1 rerun — six graph tools, ripgrep, and alternating version control](20260913-v0.2.1/REPORT.md)
- [Prior v0.2.0 expanded comparison — Graphify, Gortex, GitNexus, CodeGraph, codebase-memory-mcp, and ripgrep](20260912-expanded/REPORT.md)
- [Initial DevMap/GitNexus comparison and Rust verification](20260912-52d63a1a/REPORT.md)

The v0.2.1 report is the latest source for per-tool **positives,
negatives, and comparisons**: indexing and query latency with explicit
“× faster/slower” factors, memory and storage,
caller checks, update/deletion behavior, warnings and recovery, methodology,
unverified scenarios, and inferred DevMap improvement priorities. Its raw
measurements and audit remain linked beside the conclusions.

All use the same frozen DevCouncil corpus. Their samples were collected
separately. The v0.2.1 rerun has 261 competitive timing samples plus 82
supplemental samples alternating the preserved 0.2.0 and 0.2.1 binaries.
Keep its two campaigns and the historical timings separate. The initial run
retains its own measurements and separate Rust verification evidence.
Read each report's interface, coverage, and reproducibility limits before
comparing results. The [DevMap guide](../../../docs/devmap/README.md#benchmark-comparison)
provides a shorter summary without replacing the full report.
