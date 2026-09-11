# DevMap Rust audit — 2026-09-10

This audit targets the seven-crate DevMap kernel in `DevCouncil/rust-port`. Changes were developed against an isolated snapshot of the canonical checkout, including its existing uncommitted work. The audit patch is measured against that snapshot, not against HEAD; inherited changes are not claimed as audit work.

## Correctness and robustness contracts

| Boundary | Required behavior | Verified repair and regression evidence |
| --- | --- | --- |
| Import identity | An absolute or above-root path must not become an unrelated repository-relative file | Checked normalization returns `None`; JS/TS, shared language candidates, Terraform, Go replacement and Rust lookup propagate refusal. Valid in-root parent traversal remains supported. |
| Notebook completeness | Missing/malformed sources and capped prefixes cannot report a complete parse | Malformed containers fail; a capped prose prefix reports incomplete coverage, including the omitted count. |
| Notebook source identity | Every span and lexical owner must identify the actual code cell in raw JSON | Structural JSON offsets and one decoded/raw mapping replace whole-document text search. Escapes, duplicate keys, source-array chunks and copied code in outputs/prose are covered. |
| Notebook input ceiling | A direct public extraction call obeys the same raw-source bound | Oversized input is refused before JSON parsing or invoking the callback. |
| Notebook cache identity | Grammar changes cannot reuse an incompatible clean payload | One kernel table drives parser selection and grammar identity; extraction schema is 47. |
| Notebook execution units | Syntax cannot continue across cell boundaries; valid names can resolve between cells | Independent cell parses share the production deadline and merge one notebook payload. Raw positions distinguish scopes whose parser offsets repeat. |
| Python required suites | An empty/comment-only required body cannot be labeled clean | Structural validation supplements the grammar's permissive empty `block` node. Direct and notebook regressions cover function, async, exception and match suites; pass/docstrings remain valid. |
| Query interpretation | UTF-8 and quoted literal contents are preserved; unsupported predicates cannot be silently dropped | ASCII byte-keyword scanning, quote-aware clause boundaries and explicit refusal of repeated/empty predicates and malformed patterns. |
| Store validation | Invalid confidence is rejected even when the store is empty | Canonical finite-range validator precedes early query results. |
| Store transactions | Unrepresentable IDs cannot commit or return a wrapped successful ID | Checked generation and path conversions happen inside rollback-on-drop transactions; path interning reuses the canonical owner under an immediate transaction. |
| Watcher uncertainty | An unreadable changed path cannot disappear as if ignored | Undecidable nonignored paths enter durable classification/retry. |
| Watcher shutdown | Accepted events and overflow survive shutdown on every provider model | The bounded queue is drained, late producers are allowed to disconnect, and uncertainty leaves a durable whole-tree rescan. Continuous events do not extend the absolute deadline. |
| Frame ingestion | Every chunk, including the final newline-bearing chunk, obeys the frame ceiling | Limit checked before allocation; oversized tails are counted with saturation and discarded. |
| Response lifecycle | Write errors and stalled readers cannot silently succeed or hold the session indefinitely | Supervised tasks propagate failures; a shared deadline covers the writer lock, write and flush. |
| Stream integrity | No reply follows a partially failed frame | Sticky failure state makes all subsequent writers refuse the damaged stream. |
| Query cancellation | Teardown and timeout retire pending work without suppressing required responses | Session drop cancels unfinished workers; explicit client abandonment is distinct from internal timeout. |
| Batch execution | A batch cannot multiply the call deadline by its number of requests | One deadline spans dispatch; exhausted requests receive correlated errors, and workers and registered IDs are retired. |
| Encoded HTTP metadata | Noncanonical trailing/padding bytes cannot pass tool-name matching | Complete base64 quanta, padding positions and pad bits are validated before accepting decoded metadata. |
| Release panic recovery | A blocking worker panic must reach the existing recovery path and run cleanup | Release profile uses unwinding; an actual release executable proves worker error, Drop cleanup and a subsequent successful worker. |
| Clone ranking | Capped presentation must not change ordering by actual group size | Ranking uses displayed plus omitted members; deterministic and multi-body structural-group regressions added. |
| Qualification harness | Disabled Python assertions cannot turn unexamined invariants into passing stress results | Optimized execution is refused before launching a workload; normal and optimized test invocations cover the guard. |

Every production defect above has retained pre-fix failing evidence. The Python required-suite finding and the provider/batch findings were discovered by reviewing and attacking the initial fixes. The final verification inventory identifies which evidence was rerun on the frozen result.

## Compatibility and security impact

No dependencies, database schema migration, authentication policy, authorization policy, cryptography or access expansion were added. Input interpretation, resource bounds and failure behavior are tighter. Notebook extraction schema 46 -> 47 deliberately invalidates old extraction payloads; SQLite store schema remains 20.

`devmap_resolve::importpath::normalize_rel` now returns `Option<String>`: Rust callers must handle out-of-repository paths explicitly. All workspace callers compile. The public notebook callback signature is preserved through a thin adapter; production uses the shared-budget entry. Arbitrary caller-supplied callbacks cannot be preempted, so that adapter checks elapsed time after they return. Notebook cell ordering is static source order, not a reconstruction of interactive execution history.

An embedding workspace selects the panic profile. Hosts relying on worker-panic recovery must enable unwinding as documented in `HOST_INTEGRATION.md`. The executable probe verifies the standalone profile and is now in the three-platform CI workflow; adding the workflow step is not evidence that remote CI ran.

One inherited assertion expected Rust closure parameters to leak into the enclosing function, contradicting an existing resolver regression. It was replaced by explicit isolation plus positive typed/plain-parameter assertions. The production binding rule was preserved. An old Go replacement test that accepted any local result for an absolute path now checks explicit refusal and retains relative/remote controls.

## Evidence limits

This is a bounded source audit and adversarial qualification, not proof that all possible defects are absent. DevMap impact/affected results were explicitly partial; source review and full required checks supplemented them. The final isolated index is fresh at generation 2 with 19,587 nodes, 134,827 edges and no pending paths, but coverage remains partial: three parse failures, four pattern recoveries, one discovery refusal, four call-blind files and four import-blind files. These are corpus categories, not a count of distinct unresolved bugs. Native execution here is macOS arm64. Windows/Linux provider lifetimes were inspected in the locked dependency and simulated deterministically, but those platforms require native reruns of the final source. No installed CLI, consumer vendored kernel, release asset, tag, or production store was changed by qualification.

The registry still declares unavailable/disabled grammars and call/import limitations. No new grammar dependencies were added to conceal those limits. Static graphs do not establish full semantic soundness, dynamic execution order, complete receiver inference, or every language/framework feature. The selected mutation runs and finite soaks are reported with their actual denominators; they are not workspace-wide mutation coverage or multi-day production qualification. Older ledger claims are historical unless independently established by the results below.

## Source evidence

The following anchors are relative to `rust-port/` in the qualified source. Agent reports preserve the detailed pre-fix commands and assertions.

| Area | Canonical source anchors |
| --- | --- |
| Import containment | `crates/devmap-resolve/src/importpath.rs:213`; propagation in `crates/devmap-resolve/src/resolver.rs` |
| Notebook parsing and projection | `crates/devmap-extract/src/notebook.rs:360`, `:383`, `:672`; `tests/notebook_completeness.rs` in that crate |
| Python required suites | `crates/devmap-extract/src/treesitter.rs:780` |
| Cache identity | `crates/devmap-extract/src/cache.rs:351`, `:426` |
| Query parsing | `crates/devmap-query/src/cypher.rs:86`, `:124`, `:265` |
| Store validation/rollback | `crates/devmap-store/src/db.rs:3343`, `:4097`, `:7150`; `tests/adversarial_store.rs` in that crate |
| Watcher classification and shutdown | `crates/devmap-serve/src/watcher.rs:339`, `:525` |
| MCP batch/frame/response lifecycle | `crates/devmap-serve/src/mcp.rs:2026`, `:2399`, `:2487`, `:2628` |
| HTTP encoded metadata | `crates/devmap-serve/src/mcp_http.rs:405` |
| Clone ranking | `crates/devmap-analyze/src/clones.rs:230` |
| Release panic recovery | `Cargo.toml:30`; `crates/devmap-serve/examples/worker_recovery_probe.rs`; `verify.sh`; `../.github/workflows/rust-port.yml` |
| Stress-oracle integrity | `tools/worktree_stress.py:80`; `tools/test_worktree_stress.py` |

Cargo tests ignore the profile's panic strategy, which is why the release executable is a separate gate. See the [Cargo profile documentation](https://doc.rust-lang.org/cargo/reference/profiles.html). Encoded-header canonicalization was checked against [RFC 4648](https://www.rfc-editor.org/rfc/rfc4648); malformed metadata now receives an explicit HTTP refusal.


## Final qualification

**Verified:** 2,543 workspace tests passed across 230 result groups; strict workspace/all-target Clippy and formatting passed. The five separate parser-free build/test lanes passed 667 tests. The all-target build linked successfully. The two normally opt-in tests were explicitly run: real Claude plugin/marketplace strict validation and the 40-cycle daemon storm. The remaining ignored entry is a writer helper actually launched by its collected parent test, not a skipped standalone acceptance test.

**Verified:** the actual release worker probe survived the intentional panic, ran Drop cleanup and executed the next worker. The final cold-store self-build took 2,840 ms over 1,708 files: 205 MiB database against the 266 MiB gate, and 862 MiB peak RSS against the 930 MiB gate. The double-build digest matched, two generations were retained, and the five-cycle incremental/cold smoke passed. The ambiguity memory-model probe passed at 105,458 milli-bytes per candidate, 100% width ratio and 109% of prediction. The smoke is not used as growth-soak evidence.

**Verified:** grammar/source fuzzing executed 9,984 mutated extractions with seed 20260910, requested eight files and 64 rounds per language, in 264.15 seconds. The selected mutation campaign covered 62 distinct generated mutants: 58 caught by assertions, four scanner nontermination timeouts, zero residual misses. Initial misses caused stronger tests and were rerun; retries are not counted as additional mutants. This is selected-function coverage, not workspace-wide mutation coverage. The optional full mutation step in `verify.sh` was not run; the targeted campaigns exercise the changed ranking and parser decision boundaries.

| Native workload | Scope | Verified results |
| --- | --- | --- |
| Concurrency | 128 linked worktrees, 128 daemons, 256 editors; 65 files and 579 symbols per graph; four rounds | 12,288 edits; 1,024 measured IPC queries; 512 metadata checks; 128 crash/restarts; 128 cold comparisons; 55.375 s |
| Larger graphs | Eight linked worktrees, eight daemons, 16 editors; 4,097 files, 135,171 symbols and 258,051 edges per graph; six rounds | 1,152 edits; 96 measured IPC queries; 32 metadata checks; 12 crash/restarts; eight cold comparisons; 166.782 s |
| Event storms | 40 cycles alternating 10,000 creations, mass directory rename, deletion/recreation, and 1,000 rapid rename loops | All cycles converge; RSS half-means 128,470 -> 124,255 KiB after warm-up (-3.3%) |
| Build growth | 40 edit/build/query/restore/build cycles on a fresh 232-file fixture copy | Every restored edge digest matches; RSS and database half-means pass the unchanged 10% threshold |
| Daemon growth | 1,500 cycles on another fresh 232-file fixture copy | Final/restored digest and stopped freshness pass; both post-warm-up RSS and database half-means pass the unchanged 10% threshold |

The capacity comparison checks full sorted node identities/kinds/spans, edge endpoints/kinds/confidence/resolution, empty pending work, SQLite integrity and foreign keys. It does not compare every auxiliary analysis table. The graphs use distinct function names; the separate ambiguity probe covers candidate fan-out. Both capacity runs use two Tokio and two Rayon workers per daemon. Query latency describes the harness queries, not all possible DevMap operations: 128-worktree p50/p95/max = 31.554/150.701/341.735 ms; larger-graph p50/p95/max = 2.153/4.135/9.778 ms. Maximum aggregate *sampled* daemon RSS was 3,298,048 KiB and 6,509,776 KiB respectively; these are not peak-RSS measurements.

## Retained failures and what they establish

The first integrated check stopped on a Clippy finding in the new source map; it was fixed before final qualification. Intermediate parallel development also produced a test-only missing import, stale extraction-schema assertion and an E0597 borrow-check error; those are retained and superseded by the final frozen-source runs. An earlier shared-target doctest run lost linked rlib artifacts; the final exclusive workspace doctests passed. A pre-existing timing assertion measured 3.12517375 s against 3 s under concurrent work; its threshold was not changed and it passed in final full and parser-free runs.

The 100-cycle daemon soak failed its DB half-mean comparison (2,106,307 -> 3,313,886 bytes); the 500-cycle run also failed (9,900,576 -> 17,000,826 bytes). Both RSS checks passed. Neither failed run is reported as a pass. The actual 16 KiB page size and SQLite's 1,000-frame auto-checkpoint threshold explain the observed WAL ramp: the 500-cycle run became exactly flat at 18,226,872 total bytes from cycle 400 through 500; after shutdown the main database was 1,671,168 bytes. Two generations remained, pending work was empty, and integrity/foreign-key checks passed. The threshold is an auto-checkpoint trigger, not a universal WAL size cap: long readers or large transactions can exceed it. The 1,500-cycle run extends the unchanged observation window beyond that measured ramp and the five-minute maintenance boundary. See `wal-soak-diagnosis.md` and every retained CSV; no threshold or production checkpoint policy was changed to obtain a passing result.

## Delivery and reproducibility

Three agents audited extraction/resolution, persistence/querying and serve/CLI boundaries; the parent covered clone analysis, release panic behavior, harness integrity, integrated validation and preparation of the patch. Cross-review produced the notebook execution-unit, watcher-provider and batch-deadline follow-ups.

The exact qualified executable SHA-256 is `bea21be24a38d90f0024cf6d0638996c85491fc1b392d87ce2f20c1c6156d7d8` (`devmap 0.1.1`, store schema 20). The tested-file manifest SHA-256 is `2aa584d44d8edc22a3885ff69a63f7c3d480777223946126bc4b3e0af247f41b`. Source files did not change during final qualification. This report, its JSON receipt and the STATUS entry are documentation added afterward. The integration preimage check refused before applying any files: concurrent MCP roots/workspace-discovery edits changed `devmap-cli/src/main.rs`, `devmap-serve/src/lib.rs` and `devmap-serve/src/mcp.rs`; `devmap-cli/src/claude.rs` also changed before the follow-up snapshot. The qualified changes remain in this isolated worktree and the standalone audit patch. Canonical integration is open: the active MCP changes must reach a stable snapshot, be merged with this audit, and pass a new combined-source qualification. The existing test results do not qualify that newer concurrent implementation. See `integration-receipt.json` and `concurrent-drift.json`; zero canonical files were applied and no active edits were overwritten. No commit, staging, release, installation or consumer cutover is included.

The 1,500-cycle daemon run lasted 464.555 seconds. The post-warm-up windows were cycles 400–949 and 950–1,500: RSS means were 27,099,136 -> 27,199,789 bytes; database-plus-WAL means were 18,520,785 -> 12,592,325 bytes. Shutdown left a 1,687,552-byte database, zero WAL bytes, two generations and no pending paths; independent SQLite integrity and foreign-key checks passed. The elapsed run crossed one nominal five-minute maintenance interval; maintenance execution counts were not instrumented. This does not establish indefinite stability or behavior under long-lived database readers.

- [final-index-status.json](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/final-index-status.json)
- [verify-final.log](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/verify-final.log)
- [parser-free-final.log](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/parser-free-final.log)
- [additional-ci-final.log](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/additional-ci-final.log)
- [capacity-128-final.json](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/capacity-128-final.json)
- [capacity-large-final.json](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/capacity-large-final.json)
- [build-soak-final.log](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/build-soak-final.log)
- [daemon-soak-1500-final.log](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/daemon-soak-1500-final.log)
- [wal-soak-diagnosis.md](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/wal-soak-diagnosis.md)
- [audit-only.patch](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/audit-only.patch)
- [concurrent-drift.json](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/concurrent-drift.json)
- [integration-receipt.json](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/integration-receipt.json)
- [evidence-files.json](/Users/bharath/Documents/Codex/2026-09-10/devmap-rust-audit/evidence/evidence-files.json)

The machine-readable [qualification receipt](RUST_AUDIT_2026-09-10.json) includes commands, environment, complete capacity measurements and soak half-means. The retained source tree is on `codex/devmap-rust-audit-20260910`; the separate audit patch excludes all inherited changes.
