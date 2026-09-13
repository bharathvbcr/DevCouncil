# DevMap expanded competitor benchmark — 2026-09-12

Task `52d63a1a-266d-4a5a-8a09-9b8458a98334`, revision 3. Repository: DevCouncil. The user confirmed Graphify and Gortex and requested GitNexus plus additional competitors. CodeGraph, codebase-memory-mcp, and a ripgrep text baseline complete this run. The task supplied no acceptance criteria.

**Verified outcome:** DevMap had the lowest cold-index and unchanged-refresh medians in this local CLI comparison. CodeGraph had the lowest one-file edit median. DevMap and codebase-memory-mcp returned all five inspected direct caller pairs; Gortex returned four by default and all five when its documented lower-confidence inclusion was enabled. These results describe this fixed corpus and these interfaces, not overall product superiority.

**Positives, negatives, and comparisons.** The observations below are verified against the retained run; the stated implications apply to this workload. A lower latency or smaller store does not establish equivalent analysis, and a missed selected caller does not establish a tool-wide accuracy rate.

| Tool | Positives observed | Negatives and comparison with the other tools |
|---|---|---|
| DevMap | Lowest cold-index and unchanged-refresh medians, lowest sampled cold process-tree RSS, and lowest medians for all six measured search/caller query cells. Returned 5/5 inspected caller pairs and removed the deleted probe after an ordinary rebuild. Its status and query output expose freshness and coverage limits. | CodeGraph had lower edit latency: 376.5 ms versus 817.4 ms. The 138.8 MiB store was larger than Graphify (42.5 MiB), CodeGraph (63.1 MiB), and CBM (67.1 MiB). The 91,784 remaining unresolved attribution sites, SQL import gaps, and PowerShell fallback prevent a complete-coverage claim. The preserved dirty binary lacks its exact source patch. |
| CodeGraph | Lowest single-file edit median, second-lowest cold-index median, a smaller store than DevMap, stable Go/Python caller answers, and successful update/deletion controls. | Returned 3/5 inspected caller pairs, missing both Rust test callers. Cold process-tree RSS was 2422.1 MiB versus DevMap’s 611.5 MiB; its search/caller CLI medians were higher. The failed initial `index` attempt required the documented `init` setup and was excluded from timing results. |
| codebase-memory-mcp (CBM) | Matched DevMap’s 5/5 inspected caller pairs, including both Rust tests; all freshness controls passed. Its 67.1 MiB store was smaller than DevMap’s and its 1217.2 MiB sampled cold RSS was the second-lowest among the graph tools. Native output disclosed partial parsing and exclusions. | Cold, unchanged, edit, and query CLI medians were higher than DevMap’s. Queries took about four seconds through the standalone CLI; persistent MCP latency was not measured, and this experiment did not isolate the source of that overhead. Ten files were partially parsed and vendor directories were excluded. |
| Graphify | Smallest measured index/cache directory among graph tools, successful definition/update/deletion controls, and all three inspected Go/Python caller pairs. The measured AST-only mode completed without hosted model inference. | Returned 3/5 caller pairs, missing both Rust callers. Its 21.674 s cold median and 3293.4 MiB sampled cold RSS exceeded DevMap’s; the latter was the largest observed. Unchanged extraction repeatedly re-queued 232 files with no nodes. Documentation, clustering, and LLM enrichment were excluded, so these results do not evaluate its full feature set. |
| Gortex | Returned 4/5 caller pairs by default and 5/5 with documented name-only inclusion, preserving inferred labels. Resident query medians were 72.7–94.6 ms; edits were visible, and exact-ID lookups established deletion and absence. It exposes separate query-ready and enrichment-complete gates. | Query-ready took 12.072 s and the full cold invocation took 29.526 s; the resident-daemon interface differs from the standalone CLIs. Its 343.4 MiB database/WAL was the largest measured. Default confidence filtering hid one inspected Rust caller, capped fuzzy searches could not prove absence, and the run recorded a prune timeout and unavailable rust-analyzer. The 33.671 s restore and first-refresh outlier remain part of the findings. |
| GitNexus | Found all three definitions, all three inspected Go/Python caller pairs, and every inserted probe. Unchanged refresh was much cheaper than its cold rebuild. Context responses included source and relationships, and a documented forced rebuild recovered the stale index. | Returned 3/5 caller pairs and retained the deleted probe across two normal refreshes. It had the highest cold and edit medians here, with more cold RSS and index storage than DevMap. Its missing COBOL parser is an environment/coverage limitation; the richer context output also prevents an equal-output query-speed claim. |
| ripgrep | Roughly 31–33 ms literal searches without building a graph index; a useful independent check that the source text exists. | Occurrences do not identify exact semantic definitions, direct callers, confidence tiers, or change impact. Its text-search timings are a separate baseline, not a competing graph-accuracy score. |

Evidence for these comparisons is in the timing, memory, answer, and coverage sections below; [summary.json](summary.json) and [audit.json](audit.json) preserve the machine-readable values.

**Indexing latency.** Median (observed min–max). Three cold and edited-file repetitions; five unchanged refreshes.

| Tool | Cold index | Unchanged refresh | One tracked file edited |
|---|---:|---:|---:|
| DevMap | 1.971 s (1.919 s–2.460 s) | 74.2 ms (61.0 ms–82.4 ms) | 817.4 ms (707.9 ms–1.094 s) |
| CodeGraph | 2.900 s (2.753 s–2.904 s) | 211.2 ms (179.8 ms–253.3 ms) | 376.5 ms (339.8 ms–1.181 s) |
| codebase-memory-mcp | 7.790 s (7.631 s–8.015 s) | 5.907 s (5.697 s–6.240 s) | 8.898 s (8.006 s–9.692 s) |
| Graphify | 21.674 s (16.486 s–23.690 s) | 3.804 s (3.549 s–4.528 s) | 3.764 s (3.573 s–3.787 s) |
| Gortex | 29.526 s (28.527 s–31.564 s) | 299.8 ms (288.6 ms–3.833 s) | 3.923 s (3.893 s–3.936 s) |
| GitNexus | 30.051 s (28.969 s–30.129 s) | 508.6 ms (446.4 ms–652.3 ms) | 31.792 s (30.061 s–32.406 s) |

Gortex cold measurements include daemon startup through its “enrichment complete” log gate. Its separately recorded median was **12.072 s to query-ready** and **29.478 s to enrichment complete**; wrapper/shutdown overhead explains the small difference from the table. Warm/edit/query measurements use the resident daemon. The first unchanged Gortex refresh took 3.833 s; the spread is retained. Its restore operation took 33.671 s and logged an analysis-generation prune timeout. See [Gortex lifecycle evidence](gortex-cold-3.log).

**Answer checks.** Three definitions at exact source paths and five inspected caller pairs across Rust, Go, and Python, repeated five times. Every tool found all three definitions in all repetitions. Caller sets were stable.

| Tool | Default caller pairs returned | All three edits visible | Deleted probe absent after normal restore |
|---|---:|---|---|
| DevMap | 5/5 | Yes | Yes |
| CodeGraph | 3/5 | Yes | Yes |
| codebase-memory-mcp | 5/5 | Yes | Yes |
| Graphify | 3/5 | Yes | Yes |
| Gortex | 4/5 | Yes | Yes, exact-ID follow-up |
| GitNexus | 3/5 | Yes | No; forced rebuild required |

The inspected source cases were:

| Definition | Definition file | Expected direct callers |
|---|---|---|
| `db_size_gate_bytes` | `rust/devmap-extract/src/model.rs` | `size_gate_constants_are_their_declared_magnitudes` in the same file; `db_size_gate_scales_with_file_count_and_keeps_a_floor` in `rust/devmap-cli/tests/test_scholarlm_findings.rs` |
| `loadBounded` | `backend/go_orchestrator/repomap/repomap.go` | `Load` in the same file; `TestAnInternedGraphIsHeldToTheSameBound` in `backend/go_orchestrator/repomap/compact_test.go` |
| `describe_corpus` | `benchmarks/map_bench.py` | `main` in the same file |

The Rust case is `db_size_gate_bytes`, called inside assertions by two test functions. Graphify, GitNexus, and CodeGraph returned neither caller. Gortex returned the same-file caller and suppressed five name-only edges for the other caller. A supplemental `get_callers` request with `min_tier:"text_matched"` returned both expected Rust caller pairs, bringing its inspected total to 5/5; those extra edges remain labeled inferred. See [source evidence](caller-source-evidence.json), [audited answers](audit.json), and [Gortex opt-in response](gortex-rust-callers-include-name-only.stdout). This small selected set is not a representative accuracy estimate or a full recall/precision evaluation.

GitNexus returned the deleted `devmap_competition_revision_3` after the file was restored. A second ordinary `analyze --index-only` still returned it; `--force` cleared it. The original failing metadata and responses were preserved before recovery: [normal refresh](gitnexus-repeat-normal-refresh.stdout), [stale answer](gitnexus-repeat-stale-query.stdout), [forced recovery](gitnexus-force-recovery.stdout), [recovered answer](gitnexus-after-force-query.stdout).

Gortex’s missing-name searches produced capped fuzzy result sets (62/105 and 70/102 returned), which cannot establish absence. Exact `get_symbol` follow-ups returned `condition:"symbol_not_found"` for the deleted and nonexistent IDs, while a known positive ID succeeded. The original capped answers remain recorded as inconclusive; the exact follow-ups resolve the controls. See [removed-ID response](gortex-exact-removed.stdout) and [positive control](gortex-exact-present.stdout).

**Query latency.** Medians over five repetitions per cell. “Search” uses each native definition lookup; “callers” extracts direct callers from the tool’s native response. Gortex includes client-to-daemon transport; the others start a standalone CLI process.

| Tool | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |
|---|---:|---:|---:|---:|---:|---:|
| DevMap | 22.0 ms | 22.5 ms | 22.3 ms | 38.1 ms | 38.1 ms | 38.0 ms |
| CodeGraph | 108.4 ms | 107.1 ms | 113.7 ms | 107.6 ms | 106.3 ms | 111.8 ms |
| codebase-memory-mcp | 3.970 s | 3.970 s | 3.960 s | 4.003 s | 3.970 s | 3.972 s |
| Graphify | 437.6 ms | 441.8 ms | 431.7 ms | 198.9 ms | 196.9 ms | 196.6 ms |
| Gortex | 94.6 ms | 83.5 ms | 77.6 ms | 87.7 ms | 77.5 ms | 72.7 ms |
| GitNexus | 806.4 ms | 800.6 ms | 812.8 ms | 804.3 ms | 801.4 ms | 810.6 ms |

Ripgrep searches literal occurrences across the corpus and provides no graph traversal. Its medians were db_size_gate_bytes: 31.8 ms, loadBounded: 33.3 ms, describe_corpus: 31.0 ms.

Outputs differ substantially: DevMap `explore` includes a broader neighborhood, Graphify `explain` includes connections, and GitNexus `context --content` includes source. CodeGraph `callers` and Gortex depth-one traversal are narrower. codebase-memory-mcp’s CLI opens a fresh process for each query; these seconds-long CLI measurements do not measure its persistent MCP/server engine latency. Empty Rust caller responses are correctness misses in this sample, not successful fast traversal.

**Memory and storage.** Median cold-run sampled aggregate process-tree RSS in MiB, including workers; index bytes are logical file sizes. Gortex memory includes its resident daemon and descendants.

| Tool | Sampled process-tree peak RSS | Cold index storage |
|---|---:|---:|
| DevMap | 611.5 MiB | 138.8 MiB |
| CodeGraph | 2422.1 MiB | 63.1 MiB |
| codebase-memory-mcp | 1217.2 MiB | 67.1 MiB |
| Graphify | 3293.4 MiB | 42.5 MiB |
| Gortex | 2119.7 MiB | 343.4 MiB |
| GitNexus | 3192.9 MiB | 264.5 MiB |

RSS samples run roughly every 50 ms plus `ps` overhead, can miss short peaks, and double-count shared pages. The raw record also retains BSD `time` RSS. That field alone misses codebase-memory-mcp’s supervised worker and can miss an un-waited daemon; it is not used for the memory comparison. Storage includes Graphify extraction caches, GitNexus `.gitnexus`, CodeGraph `.codegraph`, the CBM cache root, and the Gortex database/WAL, but excludes tool installations, language/model caches, old cold-run backups, and corpus Git objects. These directories contain different kinds of data.

**Protocol and scope.** All six tools received separate clones of the same 1,186 tracked regular files (46,940,219 bytes), commit `ee07c183b127e7291c38f993f4d71ca75f1121dc`. Every file was SHA-256 verified before and after, and the edited Python file was restored. The machine was an Apple M5 Pro, 18 logical CPUs, 64 GiB RAM, macOS 27 arm64, shared with desktop activity. Cold means an empty application index; OS, compiler, install, and pilot caches were not flushed. Main tool order alternated; Gortex ran separately to avoid a background indexer contending with the standalone tools.

| Tool/version | Mode used |
|---|---|
| DevMap 0.2.0, schema 20 | Preserved task-start binary; standalone `build`, `search`, `explore --budget 8000` |
| [Graphify 0.9.59](https://github.com/Graphify-Labs/graphify) (`graphifyy`) | Core Python installation; `extract --code-only --no-cluster`, `explain`, `affected --relation calls --depth 1` |
| [Gortex 0.64.3](https://github.com/zzet/gortex/tree/v0.64.3) | Official arm64 binary; isolated SQLite daemon, default local type enrichment, `reindex_repository`, `query symbol/callers`; no `--embeddings` |
| [GitNexus 1.6.9](https://github.com/abhigyanpatwari/GitNexus) | Existing CLI; `analyze --index-only`, default embeddings disabled, `context --content` |
| [CodeGraph 1.6.0](https://github.com/colbymchenry/codegraph/tree/v1.6.0) | Official arm64 bundle; `init --yes` for cold indexing, `sync`, `query`, `callers` |
| [codebase-memory-mcp 0.10.8](https://github.com/DeusData/codebase-memory-mcp/tree/v0.10.8) | Official arm64 binary; standalone `cli index_repository --mode full --persistence false`, `search_graph`, `trace_path --include-tests true` |
| ripgrep 15.1.0 | `rg --json --fixed-strings` over the same corpus |

Graphify’s AST-only scope skips documentation and clustering; it is not its LLM-enriched mode. Gortex logs explicitly report embeddings disabled, but local language enrichment remains enabled. No hosted-model evaluation or coding-agent task-completion benchmark was run. Competitor binaries and Python packages were installed only into the task scratch directory; no product dependency manifest or global agent registration was changed.

**Coverage limits and observed warnings.** File/node/edge counts have different meanings and must not be treated as accuracy scores. The recorded inventories below retain each surface and stage rather than normalizing unlike taxonomies.

| Tool and evidence stage | Reported file measure | Symbols/nodes | Edges |
|---|---|---:|---:|
| [DevMap, cold index](cold-devmap-3.stdout) | 857 indexed files | 10,193 symbols | 33,789 |
| [Graphify, cold extraction](cold-graphify-3.stdout) | 992 code files scanned | 12,844 nodes | 34,613 |
| [CodeGraph, cold index](cold-codegraph-3.stdout) | 727 indexed files | 14,120 nodes | 52,054 |
| [CBM, cold full index](cold-cbm-3.stdout) | No comparable file total in this response | 18,632 nodes | 98,869 |
| [GitNexus, recovered metadata](gitnexus-recovered-meta.json) | 1,062 inventory files | 16,284 nodes | 47,123 |
| [Gortex, final daemon status](gortex-final-status.stdout) | 1,134 tracked-repo files | 35,041 nodes | 245,853 |
| [Gortex, final query statistics](gortex-final-stats.stdout) | 853 nodes of kind `file` | 34,212 nodes | 215,473 |

The two Gortex surfaces reported different totals. Their counting scopes and ongoing derived work were not reconciled by this experiment; the difference alone does not establish stale data or a defect. Neither total is substituted for the other. Likewise, Graphify scanned files, DevMap indexed files, and Gortex file-kind nodes are different measures.

DevMap’s inspected query envelopes report 91,784 remaining unresolved attribution sites and broader traversal omissions, despite passing the five selected caller checks. It also reports eight SQL import-extractor gaps and a PowerShell pattern fallback. Graphify repeatedly re-queued 232 manifest-stamped files with no nodes during unchanged refreshes. CBM reports 10 partially parsed files and excludes vendor directories. GitNexus reports its missing COBOL parser. Gortex could not start `rust-analyzer` (`Unknown binary ... stable-aarch64-apple-darwin`); its native Rust type provider still ran, and the opt-in caller result is explicitly name-only evidence. The Gortex prune warning and incomplete/capped answers are retained, not converted to successful complete coverage.

**What remains unverified.** This was one repository snapshot on one macOS machine, with three or five repetitions per operation. The observed ranges overlap for some comparisons, so a lower median does not mean every invocation was faster. The 261 timing samples are repeated measurements of a small workload, not 261 different repositories or correctness tasks. No confidence intervals or population-wide rankings were established.

The edit workload replaced the contents of one existing Python file with a new function, then restored the original bytes. It did not test file creation, file renaming/deletion, multi-file refactors, sustained indexing, crash recovery, or concurrent agent edits. Gortex retained its native watcher and background enrichment; its request wall time may include work outside the operation’s internally reported duration. Cold/CLI timings include process and wrapper overhead; native per-phase counters were not substituted for those end-to-end measurements.

Persistent MCP throughput/latency across all tools, semantic retrieval relevance, UI quality, embeddings/LLM-enriched modes, coding-agent task success, paid inference cost, and actual token savings were not measured. In particular, vendor cost/token-savings estimates appearing in raw output are not measured benchmark outcomes. Language and symbol taxonomies, ignored paths, query content, local language providers, and index/cache formats differ. Their file, node, edge, and storage totals are descriptive, not normalized accuracy or compression scores.

**Inferred DevMap priorities, not completed improvements.** These follow from the measurements and remain work to investigate:

1. Profile the one-file update path against CodeGraph’s lower edit median before choosing an optimization. The present results do not identify which DevMap phase causes the gap.
2. Inspect the store’s live data, indexes, retained generations, and reclaim behavior to explain the larger footprint than Graphify, CodeGraph, and CBM. The directory-size comparison alone does not establish unnecessary data.
3. Triage unresolved attribution and extractor gaps with additional source-checked cases, preserving confidence and incomplete-coverage disclosures. Matching five selected caller pairs is not a reason to remove those warnings.
4. Repeat with a clean, reproducible DevMap release build, more repositories/languages, matched provider availability and query outputs, and both standalone and persistent interfaces. Expand the edit scenarios and ground truth before making broader product claims.

**Verification and artifacts.** 261 competitive timing samples in 57 groups completed successfully. 317 total timed records include pilots, controls, diagnostics, and recovery. The failed CodeGraph `index` pilot was excluded and resolved with the documented `init` command. All six corpus copies passed the final 1,186-file hash check; the temporary daemon was stopped. The sole failed freshness check was the reproduced GitNexus normal-refresh case, subsequently recovered; Gortex’s two initially inconclusive searches were resolved by exact lookup.

The DevMap binary reports `ee07c183b127-dirty`; its SHA-256 is `e5e2dc8c19731cc98c14e675c349614da90651d176403194a27d622819c8b572`. The binary is preserved, but its exact dirty source patch was not captured at the initial task start. This is not a clean-build reproducibility claim. [Provenance](provenance.json), [corpus hashes](corpus-manifest.json), [installed binary checksums](installed-binaries.json), and [Python package versions](graphify-installed-requirements.txt) retain the available evidence.

The full Rust verification completed during the initial comparison phase: 3,028 tests passed, zero failed, three ignored, with optional mutation testing skipped. This expanded phase changed only benchmark artifacts, so that product suite was not rerun. Those test results describe the then-current checkout, not competitor qualification or proof that a different binary passes the same checks. Its earlier scope and limitations remain in the [initial report](../20260912-52d63a1a/REPORT.md). Benchmark verification consists of recorded native runs, exact-source checks, corpus hashes, audit assertions, and Python compilation.

Raw commands, cwd, wall time, load average, RSS, exit status, and output filenames are in [measurements.jsonl](measurements.jsonl). Per-operation spreads are in [summary.json](summary.json); answer checks, limits, and recovery are in [audit.json](audit.json). The scripts [run_expanded.py](run_expanded.py), [run_gortex.py](run_gortex.py), [final_checks.py](final_checks.py), and [gortex_followup.py](gortex_followup.py) reproduce the recorded workloads using the preserved [plan](plan.json). They intentionally refuse to overwrite existing cold-run directories. No commit, push, or external publication was performed.

To regenerate this documentation from the existing evidence without running any benchmark tool, use `python3 benchmarks/results/competition/20260912-expanded/write_report.py` from the repository root. The [generator](write_report.py) owns the report text; update it alongside the narrative so regeneration preserves the positives, negatives, and limitations.
