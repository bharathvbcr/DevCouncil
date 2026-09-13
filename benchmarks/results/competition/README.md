# DevMap competitor benchmarks

- [Expanded comparison — Graphify, Gortex, GitNexus, CodeGraph, codebase-memory-mcp, and ripgrep](20260912-expanded/REPORT.md)
- [Initial DevMap/GitNexus comparison and Rust verification](20260912-52d63a1a/REPORT.md)

The expanded report is the canonical source for per-tool **positives,
negatives, and comparisons**: indexing and query latency, memory and storage,
caller checks, update/deletion behavior, warnings and recovery, methodology,
unverified scenarios, and inferred DevMap improvement priorities. Its raw
measurements and audit remain linked beside the conclusions.

Both use a frozen DevCouncil corpus. Their samples were collected separately;
use the expanded run for comparisons across all six graph tools, and the
initial run for its own measurements and separate Rust verification evidence.
Read each report's interface, coverage, and reproducibility limits before
comparing results. The [DevMap guide](../../../docs/devmap/README.md#benchmark-comparison)
provides a shorter summary without replacing the full report.
