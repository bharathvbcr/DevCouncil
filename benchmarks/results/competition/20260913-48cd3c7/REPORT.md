# DevMap build 48cd3c7 competitor benchmark — 2026-09-13 UTC

Task `52d63a1a-266d-4a5a-8a09-9b8458a98334`. This is a fresh rerun of all six graph tools and the ripgrep text baseline against the **currently installed** DevMap executable. The [previous v0.2.1 report](../20260913-v0.2.1/REPORT.md) measured build `506a617`; the installed binary has since advanced 49 commits to build `48cd3c7c5a84`, so that report no longer describes the shipping executable. Both earlier reports and their raw evidence remain unchanged. The task supplied no acceptance criteria; the prior measurement protocol defines this rerun’s scope.

**Verified outcome:** DevMap 0.2.1 (48cd3c7) had the lowest cold-index median, DevMap 0.2.1 (48cd3c7) the lowest unchanged-refresh median, and CodeGraph the lowest single-file edit median. DevMap returned 5/5 inspected caller pairs. All 261 competitive timing samples completed successfully; command completion is separate from the correctness findings below.

**Binary and corpus.** DevMap reports `devmap 0.2.1 (store schema 20, code graph schema 2, build 48cd3c7c5a84)`. Its preserved executable SHA-256 is `6d9a60cdcea4ef8d9ffce70c29c238a1b6e74c84614b5ed977bc32971864224f`, and clean-build metadata identifies source commit `48cd3c7c5a84570e3047779f23d2a4c4777b5278`. The binary was copied from the installed executable and hash-verified; it was not rebuilt or independently release-signature-qualified during this run.

The benchmark corpus deliberately stays at commit `ee07c183b127e7291c38f993f4d71ca75f1121dc`: 1,186 tracked regular files, 46,940,219 bytes, independently hash-verified in six fresh clones before and after. Thus the binary build changed without replacing the source workload. [Provenance](provenance.json), [plan](plan.json), and [corpus manifest](corpus-manifest.json) retain the identities.

**Positives, negatives, and comparisons.** These are observations about this workload, interfaces, and selected source cases. Different analysis scope prevents treating smaller stores or lower latency as equivalent graph quality.

| Tool | Positives observed | Negatives and comparison limits |
|---|---|---|
| DevMap 0.2.1 (48cd3c7) | 5/5 caller pairs; 3/3 edits visible; removed probe absent: Yes. Lowest median in 6/6 graph-query cells and lowest sampled cold RSS, 610.3 MiB. Clean-build identity is recorded. | Edit median 957.8 ms versus CodeGraph’s 417.2 ms; store 138.9 MiB versus Graphify’s 42.5, CodeGraph’s 63.1, and CBM’s 67.1. 96,233 unexplained attribution sites remain; the five caller pairs are not full accuracy coverage. |
| CodeGraph | Cold 2.912 s, edit 417.2 ms, store 63.1 MiB; 3/3 edits visible and removal check: Yes. | 3/5 caller pairs; missing cases are listed below. Cold sampled RSS 2437.9 MiB. Query results and analysis differ from DevMap’s composed neighborhood output. |
| codebase-memory-mcp (CBM) | 5/5 inspected callers; 3/3 edits visible, removal check: Yes; store 67.1 MiB. | Cold 8.019 s, unchanged 6.160 s, edit 9.347 s. Standalone CLI startup is included in queries; these timings do not qualify persistent MCP latency. Native exclusions and partial parses remain below. |
| Graphify | Smallest measured store, 42.5 MiB; all three definitions found; 3/3 edits visible, removal check: Yes. AST-only extraction completed without hosted inference. | 3/5 caller pairs; cold 16.456 s, sampled RSS 3293.2 MiB. Documentation, clustering, and LLM enrichment were excluded, so full-feature performance and quality remain unmeasured. |
| Gortex | 4/5 callers by default, 5/5 with explicit name-only inclusion. 3/3 edits visible; exact-ID controls establish absence. It exposes separate query-ready and enriched gates. | Query-ready 14.908 s, full cold invocation 38.835 s; store 343.6 MiB. Resident-daemon timings differ from standalone CLIs. Name-only caller edges remain inferred; capped fuzzy negative searches cannot establish absence. |
| GitNexus | All three definitions found; 3/3 edits visible; unchanged refresh 567.5 ms versus cold 34.102 s. Context responses include source and relationships. | 3/5 caller pairs; deleted probe absent after normal restore: No. Edit 33.417 s; store 264.5 MiB. Recovery and parser limitations are preserved below. |
| ripgrep | Literal text matches without constructing a graph, providing an independent source-occurrence baseline. | Occurrences do not establish semantic definitions, direct callers, confidence, or change impact; no graph-accuracy score is assigned. |

**Indexing latency.** Median and observed min–max. Three cold builds and edits, five unchanged refreshes.

| Tool | Cold index | Unchanged refresh | One existing file edited |
|---|---:|---:|---:|
| DevMap 0.2.1 (48cd3c7) | 2.031 s (2.019 s–2.168 s) | 107.1 ms (87.9 ms–141.0 ms) | 957.8 ms (882.8 ms–1.216 s) |
| CodeGraph | 2.912 s (2.724 s–3.073 s) | 241.9 ms (192.5 ms–269.0 ms) | 417.2 ms (380.5 ms–503.3 ms) |
| codebase-memory-mcp | 8.019 s (7.830 s–8.121 s) | 6.160 s (5.724 s–14.964 s) | 9.347 s (8.218 s–9.959 s) |
| Graphify | 16.456 s (16.430 s–19.123 s) | 4.327 s (3.780 s–5.924 s) | 3.945 s (3.660 s–6.247 s) |
| Gortex | 38.835 s (35.769 s–39.098 s) | 967.5 ms (879.3 ms–5.684 s) | 5.754 s (5.679 s–6.371 s) |
| GitNexus | 34.102 s (29.006 s–37.026 s) | 567.5 ms (475.1 ms–687.1 ms) | 33.417 s (30.873 s–34.542 s) |

**DevMap speed factors versus each competitor.** Each cell describes the installed DevMap build relative to the named tool, calculated from unrounded elapsed-time medians. “X× faster” means competitor time divided by DevMap time. “X× slower” means DevMap time divided by competitor time: for example, 2.30× slower means taking that multiple of the time. These ratios describe the measured native pipelines; they do not establish equivalent analysis or output.

| Compared with | Cold index | Unchanged refresh | Single-file edit |
|---|---:|---:|---:|
| CodeGraph | 1.43× faster | 2.26× faster | 2.30× slower |
| codebase-memory-mcp | 3.95× faster | 57.49× faster | 9.76× faster |
| Graphify | 8.10× faster | 40.39× faster | 4.12× faster |
| Gortex | 19.12× faster | 9.03× faster | 6.01× faster |
| GitNexus | 16.79× faster | 5.30× faster | 34.89× faster |

Gortex median internal gates were 14.908 s to query-ready and 38.501 s to enrichment complete. The cold table includes wrapper/startup/shutdown overhead. Warm/edit/query operations use its resident daemon. The first unchanged refresh took 5.684 s; restore took 34.835 s. See [lifecycle log](gortex-cold-3.log).

**Alternating build control.** After the competitor campaign, 82 supplemental timing samples alternated the preserved `506a617` binary — the executable the previous report measured — and the installed `48cd3c7` binary on the same restored corpus, using independent empty/steady stores. Order reverses by repetition. Both are version 0.2.1 and differ by 49 commits. The protocol keeps three cold builds/edits and five unchanged/query repetitions per build. Both binaries passed the three definitions, five caller pairs, three edit-visibility checks, and removed-probe controls. These samples are separate from the 261-sample competitor table.

| Operation | 506a617 median (min–max) | 48cd3c7 median (min–max) | Median change | 48cd3c7 vs 506a617 |
|---|---:|---:|---:|---:|
| cold | 2.019 s (1.957 s–2.062 s) | 1.918 s (1.901 s–2.084 s) | -5.0% | 1.05× faster |
| warm | 82.8 ms (78.0 ms–106.7 ms) | 78.8 ms (74.2 ms–84.5 ms) | -4.8% | 1.05× faster |
| edit | 786.1 ms (731.6 ms–843.7 ms) | 784.3 ms (730.8 ms–806.3 ms) | -0.2% | 1.002× faster |
| search `db_size_gate_bytes` | 23.2 ms (21.8 ms–24.0 ms) | 24.5 ms (22.1 ms–24.9 ms) | +5.6% | 1.06× slower |
| search `loadBounded` | 23.7 ms (22.3 ms–24.2 ms) | 23.1 ms (21.3 ms–24.4 ms) | -2.8% | 1.03× faster |
| search `describe_corpus` | 23.2 ms (21.3 ms–32.4 ms) | 23.4 ms (22.0 ms–25.2 ms) | +0.6% | 1.01× slower |
| callers `db_size_gate_bytes` | 39.7 ms (38.6 ms–43.1 ms) | 40.2 ms (38.7 ms–41.5 ms) | +1.2% | 1.01× slower |
| callers `loadBounded` | 41.6 ms (40.2 ms–44.6 ms) | 40.7 ms (39.2 ms–41.7 ms) | -2.1% | 1.02× faster |
| callers `describe_corpus` | 39.7 ms (38.3 ms–41.5 ms) | 40.0 ms (39.0 ms–43.4 ms) | +0.8% | 1.01× slower |

| Alternating cold control | Sampled process-tree RSS | Index size |
|---|---:|---:|
| 506a617 | 610.1 MiB | 138.9 MiB |
| 48cd3c7 | 602.3 MiB | 138.9 MiB |

The two builds produced **identical** cold store sizes in this control (145,670,144 bytes each). Alternation reduces the time-separation confound; it does not eliminate shared-desktop noise, cache/order effects, or the small sample size. This is evidence about these two preserved executables on this one corpus, not a claim that every build in that range behaves identically. [Supplemental results and audited answers](version-control.json), [runner](run_version_control.py).

**Historical DevMap comparison.** The previous report’s campaign and this one were collected at different times on a shared desktop. These deltas describe observed medians and are not controlled estimates of the effect of the build change; the alternating control above is the better-controlled comparison. Read the ranges above and the previous report’s ranges; no confidence intervals or population-wide ranking were established.

| Operation | Previous report, build 506a617 | This run, build 48cd3c7 | Median change | Current vs previous |
|---|---:|---:|---:|---:|
| cold | 2.244 s | 2.031 s | -9.5% | 1.10× faster |
| warm | 137.8 ms | 107.1 ms | -22.3% | 1.29× faster |
| edit | 1.190 s | 957.8 ms | -19.5% | 1.24× faster |
| search `db_size_gate_bytes` | 24.6 ms | 30.0 ms | +21.8% | 1.22× slower |
| search `loadBounded` | 25.2 ms | 32.5 ms | +28.8% | 1.29× slower |
| search `describe_corpus` | 26.9 ms | 31.7 ms | +18.0% | 1.18× slower |
| callers `db_size_gate_bytes` | 43.6 ms | 51.2 ms | +17.5% | 1.17× slower |
| callers `loadBounded` | 44.6 ms | 51.2 ms | +14.8% | 1.15× slower |
| callers `describe_corpus` | 42.7 ms | 48.5 ms | +13.6% | 1.14× slower |

Positive timing deltas mean slower observed medians; negative deltas mean faster. Both binaries carry clean-build metadata, so unlike the previous report’s historical row there is no unrecorded source patch here. Shared OS/compiler/package caches were not flushed, load varies, and absolute clone paths changed. These are additional limits on causal interpretation.

**Read the two comparisons together.** The historical table above shows this run’s query medians as slower than the previous report’s, but the alternating control — which measured *both* builds back to back on the same corpus — put them at 23.2 ms and 23.4 ms for definition search, against 31.7 ms for the same build during the competitor campaign. The campaign and the control disagree by more for one build across sessions than the two builds disagree within a session. That points at campaign conditions — interleaved competitor processes and desktop load — rather than at a query regression between `506a617` and `48cd3c7`. The controlled result is the one to rely on; neither establishes a population-level claim.

**Definitions, callers, and freshness.** Every tool found all three exact source definitions in all five repetitions. Caller sets were stable across the repetitions. Only the five pre-inspected direct caller pairs are scored; additional returned edges remain unjudged.

| Tool | Matched caller pairs | Edits visible | Deleted probe absent after normal restore |
|---|---:|---:|---|
| DevMap 0.2.1 (48cd3c7) | 5/5 | 3/3 | Yes |
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

- DevMap 0.2.1 (48cd3c7): none in this five-pair sample.
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
| DevMap 0.2.1 (48cd3c7) | 30.0 ms | 32.5 ms | 31.7 ms | 51.2 ms | 51.2 ms | 48.5 ms |
| CodeGraph | 144.4 ms | 162.6 ms | 165.7 ms | 150.9 ms | 162.5 ms | 167.2 ms |
| codebase-memory-mcp | 4.438 s | 4.391 s | 4.457 s | 4.398 s | 4.528 s | 4.352 s |
| Graphify | 567.0 ms | 614.5 ms | 575.4 ms | 292.0 ms | 274.1 ms | 270.6 ms |
| Gortex | 115.4 ms | 103.2 ms | 92.3 ms | 105.6 ms | 91.5 ms | 84.9 ms |
| GitNexus | 980.3 ms | 1.023 s | 996.9 ms | 963.6 ms | 1.041 s | 998.4 ms |

**DevMap query speed factors versus each competitor.** Each cell describes DevMap relative to that tool for the named operation. The same elapsed-time ratio convention applies. Different output scopes and missed callers remain part of the interpretation.

| Compared with | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |
|---|---:|---:|---:|---:|---:|---:|
| CodeGraph | 4.82× faster | 5.00× faster | 5.23× faster | 2.95× faster | 3.17× faster | 3.45× faster |
| codebase-memory-mcp | 148.19× faster | 135.06× faster | 140.63× faster | 85.86× faster | 88.50× faster | 89.77× faster |
| Graphify | 18.93× faster | 18.90× faster | 18.16× faster | 5.70× faster | 5.36× faster | 5.58× faster |
| Gortex | 3.85× faster | 3.17× faster | 2.91× faster | 2.06× faster | 1.79× faster | 1.75× faster |
| GitNexus | 32.73× faster | 31.48× faster | 31.45× faster | 18.81× faster | 20.34× faster | 20.59× faster |

Ripgrep literal search medians: `db_size_gate_bytes`: 35.7 ms, `loadBounded`: 36.4 ms, `describe_corpus`: 36.3 ms.

DevMap definition-search elapsed-time ratios against the ripgrep text baseline: `db_size_gate_bytes`: 1.19× faster, `loadBounded`: 1.12× faster, `describe_corpus`: 1.15× faster. These are different operations: exact symbol lookup against an existing graph versus literal occurrences over source text. The ratios are not a claim of equivalent retrieval.

Outputs differ: DevMap `explore` includes a broader neighborhood, Graphify `explain` includes connections, and GitNexus `context --content` includes source. CodeGraph `callers` and Gortex depth-one traversal are narrower. CBM CLI timings include opening a fresh process and are not its persistent MCP engine latency. Missing callers are correctness misses in this sample, not successful fast traversal.

**Memory and storage.** Cold-run medians. RSS samples aggregate the measured process tree and supervised workers; Gortex includes the resident daemon and descendants.

| Tool | Sampled process-tree peak RSS | Cold index/cache storage |
|---|---:|---:|
| DevMap 0.2.1 (48cd3c7) | 610.3 MiB | 138.9 MiB |
| CodeGraph | 2437.9 MiB | 63.1 MiB |
| codebase-memory-mcp | 1207.8 MiB | 67.1 MiB |
| Graphify | 3293.2 MiB | 42.5 MiB |
| Gortex | 2206.0 MiB | 343.6 MiB |
| GitNexus | 3094.9 MiB | 264.5 MiB |

RSS is sampled roughly every 50 ms plus `ps` overhead, can miss short peaks, and double-counts shared pages. Raw BSD `time` RSS is also retained but is not used for ranking because it misses CBM’s supervised worker and may omit an un-waited daemon. Storage is logical file size of the native index/cache directories (including Gortex database/WAL), excluding installations, language/model caches, archived cold stores, and corpus Git objects. Different formats and retained data make these descriptive totals, not normalized compression scores.

**Coverage inventories and warnings.** Counts preserve each native surface and stage; they are not accuracy scores.

| Native surface | Reported file measure | Symbols/nodes | Edges |
|---|---|---:|---:|
| [DevMap cold](cold-devmap-3.stdout) | 857 indexed | 10,193 | 33,789 |
| [Graphify cold](cold-graphify-3.stdout) | 992 code files scanned | 12,844 | 34,613 |
| [CodeGraph cold](cold-codegraph-3.stdout) | 727 indexed | 14,120 | 52,054 |
| [CBM cold full](cold-cbm-3.stdout) | No comparable top-level total | 18,632 | 98,869 |
| [GitNexus recovered metadata](gitnexus-recovered-meta.json) | 1,062 inventory files | 16,284 | 47,123 |
| [Gortex final daemon status](gortex-final-status.stdout) | 1134 tracked-repo files | 35041 | 245850 |
| [Gortex final query stats](gortex-final-stats.stdout) | 853 file-kind nodes | 34,212 | 215,473 |

Gortex’s status and statistics count different surfaces at separate moments; their counting scope and ongoing derived work have not been reconciled. A discrepancy alone does not establish stale data or a defect.

DevMap reports 162,590 unresolved sites, of which 66,357 are classified as explained, leaving 96,233. Build `506a617` produced exactly the same cold graph on this corpus — 857 files, 10,193 symbols, 33,789 edges, and an identical per-language resolution breakdown leaving 96,233. So the 49 intervening commits, which include framework/route extractor changes, did not alter what DevMap extracted from this workload. That is a statement about this corpus only: it does not show the extractor changes are inert on code that exercises them. The final [status](devmap-final-status.stdout) reports 8 SQL import-extractor gaps and 1 pattern-recovered file. Coverage warnings are retained even where selected queries pass.

CBM reports 10 partially parsed files and excluded directories .git, rust/vendor, rust/testdata/vendor. The examples are capped, with full counts retained. Additional current-run warnings and controls are recorded in [observations](observations.md).

**Protocol and interpretation.** The machine was Apple M5 Pro, 18 logical CPUs, 64 GiB RAM, macOS-27.0-arm64-arm-64bit-Mach-O. Other desktop work continued. Cold means an empty application index; shared OS/compiler/install/pilot caches were not flushed. Main tool order alternated by repetition; Gortex ran separately so its daemon did not contend with those measured invocations.

| Version | Native mode |
|---|---|
| DevMap 0.2.1 (48cd3c7) | Preserved binary; standalone `build`, `search`, `explore --budget 8000`; progress disabled |
| Graphify 0.9.59 (`graphifyy`) | `extract --code-only --no-cluster`, `explain`, `affected --relation calls --depth 1` |
| Gortex 0.64.3 | SQLite daemon, default local type enrichment, `reindex_repository`, `query symbol/callers`; embeddings disabled |
| GitNexus 1.6.9 | `analyze --index-only`, embeddings disabled, `context --content` |
| CodeGraph 1.6.0 | `init --yes` for cold; `sync`, `query`, `callers` |
| codebase-memory-mcp 0.10.8 | Standalone `cli index_repository --mode full --persistence false`, `search_graph`, `trace_path --include-tests true` |
| ripgrep 15.1.0 | `rg --json --fixed-strings` over the same corpus |

Competitor installation hashes and Graphify dependency versions were checked against the prior run. They were reused locally; no new dependencies or global agent registrations were added. Source documentation and recorded native help informed the reused flags. Full commands, cwd, elapsed time, load averages, sampled RSS, exit status, and stdout/stderr paths are in [measurements.jsonl](measurements.jsonl); spreads are in [summary.json](summary.json).

**What remains unverified.** One snapshot, one machine, three/five repetitions per operation, three definitions, and five caller pairs do not cover all languages or graph accuracy. The 261 timing samples repeat a small workload; they are not 261 repositories or distinct semantic tasks. Additional returned callers are unjudged, and no precision/recall population estimate is claimed.

The edit case appends a new function to one existing Python file, then restores the original bytes. It does not test file creation, rename/deletion, multi-file refactors, sustained watcher throughput, crash recovery, or concurrent edits. Gortex retains its watcher and background enrichment; request wall time may include work outside the native operation duration. Filters, local language providers, symbol taxonomies, and output contents differ.

Persistent MCP throughput/latency parity, semantic relevance, UI quality, embeddings/LLM enrichment, actual coding-agent completion, paid inference cost, and token savings were not measured. Vendor cost/token-saving estimates in raw outputs are not measured outcomes. No hosted inference evaluation was run. The Rust/product test suite was not rerun for this benchmark-artifact task; historical test passes in the initial report do not qualify this build.

**Inferred follow-up priorities, not completed improvements.** Profile DevMap’s edit phases before optimizing the gap with CodeGraph — it is the one stage where a competitor is consistently ahead, and the gap widened in this run. Examine store contents and reclaim behavior before calling its larger directory wasted space. Add a corpus that actually exercises the changed framework/route extractors: this snapshot produced a byte-identical graph across 49 commits of extractor work, so it cannot detect regressions or improvements in exactly the code that changed. Expand to more corpora, languages, edit types, matched provider availability/output, and persistent interfaces before making general product claims.

**Reproduction and evidence.** See [PLAN.md](PLAN.md), [preparation script](prepare_rerun.py), [reused-script hashes](reused-artifacts.json), [binary hashes](installed-binaries.json), [Graphify dependencies](graphify-installed-requirements.txt), and [source verification](all-corpora-final-verification.json). The prepared corpus and executable are under the scratch path in [plan.json](plan.json). Existing cold state is never overwritten by the preparation workflow.

Run the scripts serially as described in PLAN.md. For report-only regeneration from retained results, use `python3 benchmarks/results/competition/20260913-48cd3c7/write_report.py` from the repository root. The generator owns this narrative; [observations.md](observations.md) records directly inspected native warnings. No commit, push, or external publication is part of this run.
