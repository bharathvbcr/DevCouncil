# DevMap v0.2.1 competitor benchmark — 2026-09-13 UTC

Task `52d63a1a-266d-4a5a-8a09-9b8458a98334`. This is a fresh rerun of all six graph tools and the ripgrep text baseline. The [previous v0.2.0 report](../20260912-expanded/REPORT.md) and its raw evidence remain unchanged. The task supplied no acceptance criteria; the prior measurement protocol defines this rerun’s scope.

**Verified outcome:** DevMap 0.2.1 had the lowest cold-index median, DevMap 0.2.1 the lowest unchanged-refresh median, and CodeGraph the lowest single-file edit median. DevMap returned 5/5 inspected caller pairs. All 261 competitive timing samples completed successfully; command completion is separate from the correctness findings below.

**Binary and corpus.** DevMap reports `devmap 0.2.1 (store schema 20, code graph schema 2, build 506a617498a5)`. Its preserved executable SHA-256 is `0cf3428a85de9099d6ae30172d4f149b57c3135bcaaaaaad9ec5f73d5a72541f`, and clean-build metadata identifies source commit `506a617498a5f1dae8a89519f91b4ee67031e352`. The binary was copied from the installed executable and hash-verified; it was not rebuilt or independently release-signature-qualified during this run.

The benchmark corpus deliberately stays at commit `ee07c183b127e7291c38f993f4d71ca75f1121dc`: 1,186 tracked regular files, 46,940,219 bytes, independently hash-verified in six fresh clones before and after. Thus the binary version changed without replacing the source workload. [Provenance](provenance.json), [plan](plan.json), and [corpus manifest](corpus-manifest.json) retain the identities.

**Positives, negatives, and comparisons.** These are observations about this workload, interfaces, and selected source cases. Different analysis scope prevents treating smaller stores or lower latency as equivalent graph quality.

| Tool | Positives observed | Negatives and comparison limits |
|---|---|---|
| DevMap 0.2.1 | 5/5 caller pairs; 3/3 edits visible; removed probe absent: Yes. Lowest median in 6/6 graph-query cells and lowest sampled cold RSS, 608.0 MiB. Clean-build identity is recorded. | Edit median 1.190 s versus CodeGraph’s 691.9 ms; store 138.9 MiB versus Graphify’s 42.5, CodeGraph’s 63.1, and CBM’s 67.1. 96,233 unexplained attribution sites remain; the five caller pairs are not full accuracy coverage. |
| CodeGraph | Cold 3.267 s, edit 691.9 ms, store 63.1 MiB; 3/3 edits visible and removal check: Yes. | 3/5 caller pairs; missing cases are listed below. Cold sampled RSS 2424.9 MiB. Query results and analysis differ from DevMap’s composed neighborhood output. |
| codebase-memory-mcp (CBM) | 5/5 inspected callers; 3/3 edits visible, removal check: Yes; store 67.1 MiB. | Cold 8.453 s, unchanged 6.015 s, edit 9.259 s. Standalone CLI startup is included in queries; these timings do not qualify persistent MCP latency. Native exclusions and partial parses remain below. |
| Graphify | Smallest measured store, 42.5 MiB; all three definitions found; 3/3 edits visible, removal check: Yes. AST-only extraction completed without hosted inference. | 3/5 caller pairs; cold 17.530 s, sampled RSS 3297.3 MiB. Documentation, clustering, and LLM enrichment were excluded, so full-feature performance and quality remain unmeasured. |
| Gortex | 4/5 callers by default, 5/5 with explicit name-only inclusion. 3/3 edits visible; exact-ID controls establish absence. It exposes separate query-ready and enriched gates. | Query-ready 14.591 s, full cold invocation 33.982 s; store 343.3 MiB. Resident-daemon timings differ from standalone CLIs. Name-only caller edges remain inferred; capped fuzzy negative searches cannot establish absence. |
| GitNexus | All three definitions found; 3/3 edits visible; unchanged refresh 636.6 ms versus cold 35.103 s. Context responses include source and relationships. | 3/5 caller pairs; deleted probe absent after normal restore: No. Edit 33.760 s; store 264.5 MiB. Recovery and parser limitations are preserved below. |
| ripgrep | Literal text matches without constructing a graph, providing an independent source-occurrence baseline. | Occurrences do not establish semantic definitions, direct callers, confidence, or change impact; no graph-accuracy score is assigned. |

**Indexing latency.** Median and observed min–max. Three cold builds and edits, five unchanged refreshes.

| Tool | Cold index | Unchanged refresh | One existing file edited |
|---|---:|---:|---:|
| DevMap 0.2.1 | 2.244 s (1.890 s–2.428 s) | 137.8 ms (94.4 ms–190.8 ms) | 1.190 s (884.3 ms–1.284 s) |
| CodeGraph | 3.267 s (2.846 s–3.493 s) | 226.2 ms (199.3 ms–469.4 ms) | 691.9 ms (405.5 ms–849.4 ms) |
| codebase-memory-mcp | 8.453 s (7.969 s–8.537 s) | 6.015 s (5.855 s–8.963 s) | 9.259 s (9.038 s–9.724 s) |
| Graphify | 17.530 s (16.396 s–19.131 s) | 4.183 s (3.724 s–7.551 s) | 4.499 s (3.987 s–4.689 s) |
| Gortex | 33.982 s (33.797 s–41.804 s) | 746.6 ms (726.5 ms–4.550 s) | 4.910 s (4.897 s–5.025 s) |
| GitNexus | 35.103 s (30.903 s–39.832 s) | 636.6 ms (504.5 ms–1.217 s) | 33.760 s (30.775 s–58.404 s) |

**DevMap speed factors versus each competitor.** Each cell describes DevMap 0.2.1 relative to the named tool, calculated from unrounded elapsed-time medians. “X× faster” means competitor time divided by DevMap time. “X× slower” means DevMap time divided by competitor time: for example, 1.72× slower means taking 1.72× as long. These ratios describe the measured native pipelines; they do not establish equivalent analysis or output.

| Compared with | Cold index | Unchanged refresh | Single-file edit |
|---|---:|---:|---:|
| CodeGraph | 1.46× faster | 1.64× faster | 1.72× slower |
| codebase-memory-mcp | 3.77× faster | 43.64× faster | 7.78× faster |
| Graphify | 7.81× faster | 30.35× faster | 3.78× faster |
| Gortex | 15.14× faster | 5.42× faster | 4.13× faster |
| GitNexus | 15.64× faster | 4.62× faster | 28.38× faster |

Gortex median internal gates were 14.591 s to query-ready and 33.738 s to enrichment complete. The cold table includes wrapper/startup/shutdown overhead. Warm/edit/query operations use its resident daemon. The first unchanged refresh took 4.550 s; restore took 34.165 s. See [lifecycle log](gortex-cold-3.log).

**Alternating version control.** After the competitor campaign, 82 supplemental timing samples alternated the preserved 0.2.0 dirty binary and the 0.2.1 binary on the same restored corpus, using independent empty/steady stores. Order reverses by repetition. The protocol keeps three cold builds/edits and five unchanged/query repetitions per version. Both binaries passed the three definitions, five caller pairs, three edit-visibility checks, and removed-probe controls. These samples are separate from the 261-sample competitor table.

| Operation | 0.2.0 median (min–max) | 0.2.1 median (min–max) | Median change | 0.2.1 vs 0.2.0 |
|---|---:|---:|---:|---:|
| cold | 2.256 s (2.232 s–2.606 s) | 2.450 s (2.402 s–2.527 s) | +8.6% | 1.09× slower |
| warm | 88.6 ms (74.1 ms–97.3 ms) | 76.0 ms (73.4 ms–86.3 ms) | -14.3% | 1.17× faster |
| edit | 856.3 ms (780.1 ms–871.2 ms) | 836.3 ms (816.3 ms–848.4 ms) | -2.3% | 1.02× faster |
| search `db_size_gate_bytes` | 25.6 ms (24.5 ms–26.4 ms) | 26.4 ms (25.6 ms–27.8 ms) | +3.0% | 1.03× slower |
| search `loadBounded` | 25.5 ms (24.2 ms–28.0 ms) | 25.4 ms (24.4 ms–28.7 ms) | -0.4% | 1.004× faster |
| search `describe_corpus` | 26.0 ms (24.3 ms–26.9 ms) | 25.9 ms (25.3 ms–29.2 ms) | -0.1% | 1.001× faster |
| callers `db_size_gate_bytes` | 44.9 ms (42.7 ms–45.5 ms) | 45.0 ms (43.7 ms–46.3 ms) | +0.3% | 1.003× slower |
| callers `loadBounded` | 42.2 ms (42.0 ms–48.9 ms) | 45.0 ms (44.6 ms–49.2 ms) | +6.6% | 1.07× slower |
| callers `describe_corpus` | 44.8 ms (43.5 ms–45.6 ms) | 45.4 ms (43.4 ms–46.0 ms) | +1.5% | 1.01× slower |

| Alternating cold control | Sampled process-tree RSS | Index size |
|---|---:|---:|
| 0.2.0 | 610.5 MiB | 138.8 MiB |
| 0.2.1 | 611.7 MiB | 138.9 MiB |

Alternation reduces the time-separation confound; it does not eliminate shared-desktop noise, cache/order effects, or the small sample size. The old binary still lacks its exact dirty source patch. This is evidence about these two preserved executables, not a claim that every v0.2.0 or v0.2.1 build behaves identically. [Supplemental results and audited answers](version-control.json), [runner](run_version_control.py).

**Historical DevMap comparison.** The old and new measurements were collected at different times on a shared desktop. These deltas describe observed medians and are not controlled estimates of the effect of the version change. Read the ranges above and the old report’s ranges; no confidence intervals or population-wide ranking were established.

| Operation | Prior 0.2.0 dirty build | Current 0.2.1 clean-build metadata | Median change | Current vs historical |
|---|---:|---:|---:|---:|
| cold | 1.971 s | 2.244 s | +13.8% | 1.14× slower |
| warm | 74.2 ms | 137.8 ms | +85.8% | 1.86× slower |
| edit | 817.4 ms | 1.190 s | +45.5% | 1.46× slower |
| search `db_size_gate_bytes` | 22.0 ms | 24.6 ms | +12.0% | 1.12× slower |
| search `loadBounded` | 22.5 ms | 25.2 ms | +12.2% | 1.12× slower |
| search `describe_corpus` | 22.3 ms | 26.9 ms | +20.5% | 1.21× slower |
| callers `db_size_gate_bytes` | 38.1 ms | 43.6 ms | +14.4% | 1.14× slower |
| callers `loadBounded` | 38.1 ms | 44.6 ms | +17.0% | 1.17× slower |
| callers `describe_corpus` | 38.0 ms | 42.7 ms | +12.4% | 1.12× slower |

Positive timing deltas mean slower observed medians; negative deltas mean faster. The old binary’s exact dirty source patch was not captured. Shared OS/compiler/package caches were not flushed, load varies, and absolute clone paths changed. These are additional limits on causal interpretation.

**Definitions, callers, and freshness.** Every tool found all three exact source definitions in all five repetitions. Caller sets were stable across the repetitions. Only the five pre-inspected direct caller pairs are scored; additional returned edges remain unjudged.

| Tool | Matched caller pairs | Edits visible | Deleted probe absent after normal restore |
|---|---:|---:|---|
| DevMap 0.2.1 | 5/5 | 3/3 | Yes |
| CodeGraph | 3/5 | 3/3 | Yes |
| codebase-memory-mcp | 5/5 | 3/3 | Yes |
| Graphify | 3/5 | 3/3 | Yes |
| Gortex | 4/5 | 3/3 | Yes, exact-ID follow-up |
| GitNexus | 3/5 | 3/3 | No |

| Definition | Exact source path | Expected direct callers |
|---|---|---|
| `db_size_gate_bytes` | `rust/devmap-extract/src/model.rs` | `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes` |
| `loadBounded` | `backend/go_orchestrator/repomap/repomap.go` | `backend/go_orchestrator/repomap/compact_test.go::TestAnInternedGraphIsHeldToTheSameBound`; `backend/go_orchestrator/repomap/repomap.go::Load` |
| `describe_corpus` | `benchmarks/map_bench.py` | `benchmarks/map_bench.py::main` |

Missing caller pairs in the default responses:

- DevMap 0.2.1: none in this five-pair sample.
- CodeGraph: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes`.
- codebase-memory-mcp: none in this five-pair sample.
- Graphify: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes`.
- Gortex: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`.
- GitNexus: `rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor`; `rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes`.

Gortex’s supplemental `get_callers` request used `min_tier:"text_matched"` and `exclude_tests:false`, returning a combined 5/5 inspected pairs. This one supplemental request does not replace its default score or query timings. Exact-ID checks returned `condition:"symbol_not_found"` for deleted/nonexistent IDs and succeeded for a known positive ID. [Opt-in callers](gortex-rust-callers-include-name-only.stdout), [deleted-ID control](gortex-exact-removed.stdout), [positive control](gortex-exact-present.stdout).

GitNexus retained the deleted probe after a second ordinary refresh: **yes**. A forced rebuild cleared it: **yes**. The original responses and metadata precede recovery: [ordinary refresh](gitnexus-repeat-normal-refresh.stdout), [normal answer](gitnexus-repeat-stale-query.stdout), [forced rebuild](gitnexus-force-recovery.stdout), [recovered answer](gitnexus-after-force-query.stdout).

The five-pair source snippets are preserved in [caller-source-evidence.json](caller-source-evidence.json). The [audit](audit.json) retains every repetition, missing/additional pairs, failed/inconclusive controls, and query truncation/coverage flags. A successful command is not a successful semantic check.

**Query latency.** Medians over five repetitions per cell. Search is each native definition lookup; callers uses native response traversal. Gortex includes client-to-daemon transport, while the other graph tools start standalone processes.

| Tool | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |
|---|---:|---:|---:|---:|---:|---:|
| DevMap 0.2.1 | 24.6 ms | 25.2 ms | 26.9 ms | 43.6 ms | 44.6 ms | 42.7 ms |
| CodeGraph | 129.7 ms | 123.0 ms | 131.2 ms | 118.5 ms | 118.4 ms | 120.3 ms |
| codebase-memory-mcp | 4.214 s | 4.108 s | 4.112 s | 4.137 s | 4.196 s | 4.261 s |
| Graphify | 480.0 ms | 508.7 ms | 466.7 ms | 229.2 ms | 222.7 ms | 213.1 ms |
| Gortex | 98.7 ms | 88.0 ms | 82.2 ms | 95.1 ms | 83.6 ms | 77.2 ms |
| GitNexus | 907.5 ms | 867.2 ms | 899.7 ms | 884.5 ms | 869.4 ms | 878.5 ms |

**DevMap query speed factors versus each competitor.** Each cell describes DevMap relative to that tool for the named operation. The same elapsed-time ratio convention applies. Different output scopes and missed callers remain part of the interpretation.

| Compared with | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |
|---|---:|---:|---:|---:|---:|---:|
| CodeGraph | 5.28× faster | 4.87× faster | 4.89× faster | 2.72× faster | 2.65× faster | 2.82× faster |
| codebase-memory-mcp | 171.38× faster | 162.78× faster | 153.15× faster | 94.90× faster | 94.10× faster | 99.83× faster |
| Graphify | 19.52× faster | 20.16× faster | 17.38× faster | 5.26× faster | 4.99× faster | 4.99× faster |
| Gortex | 4.02× faster | 3.49× faster | 3.06× faster | 2.18× faster | 1.88× faster | 1.81× faster |
| GitNexus | 36.91× faster | 34.36× faster | 33.51× faster | 20.29× faster | 19.50× faster | 20.58× faster |

Ripgrep literal search medians: `db_size_gate_bytes`: 33.8 ms, `loadBounded`: 33.1 ms, `describe_corpus`: 33.1 ms.

DevMap definition-search elapsed-time ratios against the ripgrep text baseline: `db_size_gate_bytes`: 1.37× faster, `loadBounded`: 1.31× faster, `describe_corpus`: 1.23× faster. These are different operations: exact symbol lookup against an existing graph versus literal occurrences over source text. The ratios are not a claim of equivalent retrieval.

Outputs differ: DevMap `explore` includes a broader neighborhood, Graphify `explain` includes connections, and GitNexus `context --content` includes source. CodeGraph `callers` and Gortex depth-one traversal are narrower. CBM CLI timings include opening a fresh process and are not its persistent MCP engine latency. Missing callers are correctness misses in this sample, not successful fast traversal.

**Memory and storage.** Cold-run medians. RSS samples aggregate the measured process tree and supervised workers; Gortex includes the resident daemon and descendants.

| Tool | Sampled process-tree peak RSS | Cold index/cache storage |
|---|---:|---:|
| DevMap 0.2.1 | 608.0 MiB | 138.9 MiB |
| CodeGraph | 2424.9 MiB | 63.1 MiB |
| codebase-memory-mcp | 1208.9 MiB | 67.1 MiB |
| Graphify | 3297.3 MiB | 42.5 MiB |
| Gortex | 2270.8 MiB | 343.3 MiB |
| GitNexus | 2995.1 MiB | 264.5 MiB |

RSS is sampled roughly every 50 ms plus `ps` overhead, can miss short peaks, and double-counts shared pages. Raw BSD `time` RSS is also retained but is not used for ranking because it misses CBM’s supervised worker and may omit an un-waited daemon. Storage is logical file size of the native index/cache directories (including Gortex database/WAL), excluding installations, language/model caches, archived cold stores, and corpus Git objects. Different formats and retained data make these descriptive totals, not normalized compression scores.

**Coverage inventories and warnings.** Counts preserve each native surface and stage; they are not accuracy scores.

| Native surface | Reported file measure | Symbols/nodes | Edges |
|---|---|---:|---:|
| [DevMap cold](cold-devmap-3.stdout) | 857 indexed | 10,193 | 33,789 |
| [Graphify cold](cold-graphify-3.stdout) | 992 code files scanned | 12,844 | 34,613 |
| [CodeGraph cold](cold-codegraph-3.stdout) | 727 indexed | 14,120 | 52,054 |
| [CBM cold full](cold-cbm-3.stdout) | No comparable top-level total | 18,632 | 98,869 |
| [GitNexus recovered metadata](gitnexus-recovered-meta.json) | 1,062 inventory files | 16,284 | 47,123 |
| [Gortex final daemon status](gortex-final-status.stdout) | 1134 tracked-repo files | 35041 | 245852 |
| [Gortex final query stats](gortex-final-stats.stdout) | 853 file-kind nodes | 34,212 | 215,473 |

Gortex’s status and statistics count different surfaces at separate moments; their counting scope and ongoing derived work have not been reconciled. A discrepancy alone does not establish stale data or a defect.

DevMap reports 162,590 unresolved sites, of which 66,357 are classified as explained, leaving 96,233. The old run left 91,784 despite the same 10,193-symbol/33,789-edge totals; this classification difference has not been validated as a change in actual caller accuracy. The final [status](devmap-final-status.stdout) reports 8 SQL import-extractor gaps and 1 pattern-recovered file. Coverage warnings are retained even where selected queries pass.

CBM reports 10 partially parsed files and excluded directories .git, rust/vendor, rust/testdata/vendor. The examples are capped, with full counts retained. Additional current-run warnings and controls are recorded in [observations](observations.md).

**Protocol and interpretation.** The machine was Apple M5 Pro, 18 logical CPUs, 64 GiB RAM, macOS-27.0-arm64-arm-64bit-Mach-O. Other desktop work continued. Cold means an empty application index; shared OS/compiler/install/pilot caches were not flushed. Main tool order alternated by repetition; Gortex ran separately so its daemon did not contend with those measured invocations.

| Version | Native mode |
|---|---|
| DevMap 0.2.1 | Preserved binary; standalone `build`, `search`, `explore --budget 8000`; progress disabled |
| Graphify 0.9.59 (`graphifyy`) | `extract --code-only --no-cluster`, `explain`, `affected --relation calls --depth 1` |
| Gortex 0.64.3 | SQLite daemon, default local type enrichment, `reindex_repository`, `query symbol/callers`; embeddings disabled |
| GitNexus 1.6.9 | `analyze --index-only`, embeddings disabled, `context --content` |
| CodeGraph 1.6.0 | `init --yes` for cold; `sync`, `query`, `callers` |
| codebase-memory-mcp 0.10.8 | Standalone `cli index_repository --mode full --persistence false`, `search_graph`, `trace_path --include-tests true` |
| ripgrep 15.1.0 | `rg --json --fixed-strings` over the same corpus |

Competitor installation hashes and Graphify dependency versions were checked against the prior run. They were reused locally; no new dependencies or global agent registrations were added. Source documentation and recorded native help informed the reused flags. Full commands, cwd, elapsed time, load averages, sampled RSS, exit status, and stdout/stderr paths are in [measurements.jsonl](measurements.jsonl); spreads are in [summary.json](summary.json).

**What remains unverified.** One snapshot, one machine, three/five repetitions per operation, three definitions, and five caller pairs do not cover all languages or graph accuracy. The 261 timing samples repeat a small workload; they are not 261 repositories or distinct semantic tasks. Additional returned callers are unjudged, and no precision/recall population estimate is claimed.

The edit case appends a new function to one existing Python file, then restores the original bytes. It does not test file creation, rename/deletion, multi-file refactors, sustained watcher throughput, crash recovery, or concurrent edits. Gortex retains its watcher and background enrichment; request wall time may include work outside the native operation duration. Filters, local language providers, symbol taxonomies, and output contents differ.

Persistent MCP throughput/latency parity, semantic relevance, UI quality, embeddings/LLM enrichment, actual coding-agent completion, paid inference cost, and token savings were not measured. Vendor cost/token-saving estimates in raw outputs are not measured outcomes. No hosted inference evaluation was run. The Rust/product test suite was not rerun for this benchmark-artifact task; historical test passes in the initial report do not qualify this v0.2.1 binary.

**Inferred follow-up priorities, not completed improvements.** Profile DevMap’s edit phases before optimizing the gap with CodeGraph; examine store contents and reclaim behavior before calling its larger directory wasted space; inspect unresolved-attribution classification changes with more source-checked cases; and expand to more corpora, languages, edit types, matched provider availability/output, and persistent interfaces before making general product claims.

**Reproduction and evidence.** See [PLAN.md](PLAN.md), [preparation script](prepare_rerun.py), [reused-script hashes](reused-artifacts.json), [binary hashes](installed-binaries.json), [Graphify dependencies](graphify-installed-requirements.txt), and [source verification](all-corpora-final-verification.json). The prepared corpus and executable are under the scratch path in [plan.json](plan.json). Existing cold state is never overwritten by the preparation workflow.

Run the scripts serially as described in PLAN.md. For report-only regeneration from retained results, use `python3 benchmarks/results/competition/20260913-v0.2.1/write_report.py` from the repository root. The generator owns this narrative; [observations.md](observations.md) records directly inspected native warnings. No commit, push, or external publication is part of this run.
