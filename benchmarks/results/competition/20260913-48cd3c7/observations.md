# Native warnings and comparison limits

These observations come from this rerun's retained native output. They are
separate from whether a timed command completed successfully.

- **DevMap:** the cold graph contains 857 indexed files, 10,193 symbols, and
  33,789 edges. Build `506a617` produced exactly the same totals *and* an
  identical per-language resolution breakdown: 66,357 of 162,590 unresolved
  attribution sites classified as explained, leaving 96,233 in both. So the 49
  intervening commits — which include framework/route extractor changes in
  `devmap-extract/src/frameworks.rs` — did not change what DevMap extracted
  from this corpus. That is a statement about this workload only; this
  benchmark contains no case that exercises the changed route-handler
  detection, so it is not evidence that those changes are inert in general.
  The three selected source definitions and five caller pairs are the separate
  semantic checks. See [cold output](cold-devmap-3.stdout),
  [prior build's cold output](../20260913-v0.2.1/cold-devmap-3.stdout), and
  [answer audit](audit.json).
- **Graphify:** unchanged refreshes again reported re-queuing 232
  manifest-stamped code files that had no graph nodes. The message attributes
  this to prior failed extraction; this benchmark did not independently prove
  that every such file should contain a node. The AST-only mode skips
  documentation and clustering and does not evaluate LLM enrichment. It also
  reported 49 unclassified files and 1 file skipped as potentially sensitive.
  [Unchanged extraction](warm-graphify-1.stdout).
- **CodeGraph:** used its documented `init --yes` cold setup. It recorded the
  lowest single-file edit median in this run (417.2 ms versus DevMap's
  957.8 ms). Its two Rust caller misses are separate from successful indexing
  and definition lookup. [Cold output](cold-codegraph-3.stdout),
  [Rust caller output](callers-codegraph-db_size_gate_bytes-1.stdout).
- **codebase-memory-mcp:** full indexing reports 10 partially parsed files,
  with a capped list of examples, and excludes `.git`, `rust/vendor`, and
  `rust/testdata/vendor`. Its standalone CLI timings include the worker's
  lifecycle. BSD `time` captures only about 3 MiB for the supervising process,
  so the report uses the separately sampled process-tree memory measurement.
  These timings do not measure its persistent MCP engine.
  [Native full-index diagnostics](cold-cbm-3.stdout),
  [timing and memory records](measurements.jsonl).
- **GitNexus:** the first ordinary restore again left the deleted probe in the
  index, reproducing the previous run's freshness failure. Its cold stderr
  reports one skipped COBOL file because the parser's native binding is
  unavailable ("cobol parser not available"). This is a recorded environment
  limitation; no dependency was rebuilt during the comparison. The report
  separately records the second normal refresh and forced recovery.
  [Stale answer](negative-gitnexus-devmap_competition_revision_3.stdout),
  [recovery observation](gitnexus-recovery-observation.json).
- **Gortex:** its daemon status and its query statistics disagree at the end of
  the run — status reports 1,134 files / 35,041 nodes / 245,850 edges, while
  `query stats` reports 853 file-kind nodes / 34,212 nodes / 215,473 edges.
  The two surfaces count different things at separate moments and were not
  reconciled here; a discrepancy alone does not establish stale data or a
  defect. Its default caller response omitted one inspected pair that an
  explicit `min_tier:"text_matched"`, `exclude_tests:false` request returned.
  Capped fuzzy negative searches cannot establish absence, so the removal
  control was resolved by exact-ID lookup instead.
  [Final status](gortex-final-status.stdout),
  [final stats](gortex-final-stats.stdout),
  [opt-in callers](gortex-rust-callers-include-name-only.stdout).
- **Cross-tool:** every one of the 261 competitive timing samples and all 82
  supplemental samples completed successfully. Command completion is not a
  semantic pass; the caller and freshness findings above are the correctness
  results. Store sizes are logical file sizes of differently-shaped native
  indexes and are descriptive, not normalized compression scores.
