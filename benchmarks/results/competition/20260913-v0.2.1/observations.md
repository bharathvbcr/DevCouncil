# Native warnings and comparison limits

These observations come from this rerun's retained native output. They are
separate from whether a timed command completed successfully.

- **DevMap:** the cold graph contains 857 indexed files, 10,193 symbols, and
  33,789 edges, matching the previous run's totals. The new build classifies
  66,357 of 162,590 unresolved attribution sites as explained, leaving 96,233;
  the old build classified 70,806 as explained and left 91,784. Equal resolved
  graph totals and a different classification count do not by themselves
  establish a loss of caller accuracy. The three selected source definitions
  and five caller pairs are the separate semantic checks. See
  [new cold output](cold-devmap-3.stdout),
  [prior cold output](../20260912-expanded/cold-devmap-3.stdout), and
  [answer audit](audit.json).
- **Graphify:** unchanged refreshes again reported re-queuing 232 files that
  were manifest-stamped but had no graph nodes. The message attributes this
  to prior failed extraction; this benchmark did not independently prove
  that every such file should contain a node. The AST-only mode skips
  documentation and clustering and does not evaluate LLM enrichment.
  [Unchanged extraction](warm-graphify-1.stdout).
- **CodeGraph:** this rerun used its documented `init --yes` cold setup from
  the start. The failed `index` pilot belongs to the previous campaign and
  was not repeated or counted here. The two Rust caller misses are separate
  from successful indexing and definition lookup. [Cold output](cold-codegraph-3.stdout),
  [Rust caller output](callers-codegraph-db_size_gate_bytes-1.stdout).
- **codebase-memory-mcp:** full indexing reports 10 partially parsed files,
  with a capped list of examples, and excludes `.git`, `rust/vendor`, and
  `rust/testdata/vendor`. Its standalone CLI timings include the worker's
  lifecycle. BSD `time` captures only about 3 MiB for the supervising process,
  so the report uses the separately sampled process-tree memory measurement.
  These timings do not measure its persistent MCP engine.
  [Native full-index diagnostics](cold-cbm-3.stdout),
  [timing and memory records](measurements.jsonl).
- **GitNexus:** the first ordinary restore again left the deleted probe in
  the index. Its cold stderr reports one skipped COBOL file because the
  parser's native binding is unavailable. This is a recorded environment
  limitation; no dependency was rebuilt during the comparison. The report
  separately records the second normal refresh and forced recovery.
  [Stale answer](negative-gitnexus-devmap_competition_revision_3.stdout),
  [COBOL warning](cold-gitnexus-3.stderr),
  [recovery observation](gitnexus-recovery-observation.json).
- **Gortex:** the final lifecycle log contains two analysis-generation prune
  timeout warnings, for generations 1 and 2. The native Rust type provider
  completed, while a later `rust-analyzer` startup failed with
  `Unknown binary 'rust-analyzer' in official toolchain 'stable-aarch64-apple-darwin'`.
  Thus unavailable rust-analyzer is not evidence that all Rust analysis was
  unavailable. The first refresh and restore outliers remain in the report.
  Embeddings were explicitly disabled; local type enrichment was enabled.
  [Lifecycle log](gortex-cold-3.log).
- **Gortex negative queries:** the removed-probe search returned 62/105
  results and the nonexistent-name search 77/102, both capped. These fuzzy
  searches cannot prove absence. Exact-ID follow-ups and a positive-ID
  control are audited separately; capped answers remain recorded as initially
  inconclusive. The name-only Rust caller follow-up retains inferred labels
  and does not replace the default-query timing or score.
  [Removed fuzzy search](negative-gortex-devmap_competition_revision_3.stdout),
  [Missing fuzzy search](negative-gortex-devmap_competition_nonexistent_52d63a1a.stdout),
  [Supplemental audit](audit.json).
- **Gortex counts:** final daemon status reports 1,134 files, 35,041 nodes,
  and 245,852 edges; final query statistics report 853 file-kind nodes,
  34,212 total nodes, and 215,473 edges. Their scopes/background work remain
  unreconciled, so neither is substituted for the other or labeled a defect
  from the discrepancy alone. Vendor token/cost-savings estimates in the
  statistics response are not measured benchmark outcomes.
  [Daemon status](gortex-final-status.stdout),
  [Query statistics](gortex-final-stats.stdout).
- **Version comparison:** desktop load and timings changed from the earlier
  campaign. The supplemental alternating-version control reduces separation
  in time and preserves independent stores, but uses only three/five repeats
  and still shares OS caches and desktop activity. No timing confidence
  interval, broad accuracy ranking, or product-test qualification is implied.
  [Alternating results](version-control.json).

The earlier 3,028-test Rust verification belongs to the initial campaign's
then-current checkout. No product source was changed and that suite was not
rerun here; its old pass is not a test result for this v0.2.1 executable.
No UI, hosted inference, persistent-MCP parity, full language recall, actual
agent-task success, or paid-token savings were measured.
