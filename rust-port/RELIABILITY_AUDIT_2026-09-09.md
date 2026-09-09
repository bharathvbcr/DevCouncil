# DevMap coverage and robustness audit — 9 September 2026

This audit repaired reproduced defects in query completeness reporting, snapshot composition, traversal filtering, ranking, and GitPulse's response adapter. It also exercised the existing extraction, resolution, storage, daemon, IPC, and subprocess boundaries. The tested behaviors are verified locally; arbitrary-language semantic completeness and universal platform reliability are not established.

## Scope and ownership

Canonical source: DevCouncil at `4ccadc604f40e21a3a14471d0af3c7674265e15f`, with this audit's local changes. GitPulse consumer: `/Users/bharath/.codex/worktrees/d4f0/GitPulse`. Only its code-intelligence adapter and generated DevMap vendor copies were changed. The concurrent Tasks UI work was preserved. The cumulative canonical Rust source and regression diff is +1,114/-145 lines; the GitPulse adapter and its tests are +249/-67, excluding generated copies and documentation. No dependency or database migration was introduced; schema remains 20 and CLI version remains 0.1.1. At audit completion, these changes were uncommitted; no push, application release, or UI installation was performed.

The local CLI was installed using `cargo install --path crates/devmap-cli --locked --force`. The installed executable, release build, and immutable stress candidate have the same SHA-256:

```text
bd8058a07222a9158cbb283a0702889122c24e8531e94ae92d3223e90e1c0a7e
```

## Findings and fixes

| Finding | Reproduced defect and repair | Retained regression evidence |
|---|---|---|
| Coverage accounting | All unresolved attribution sites were described as missing graph edges, including explained built-ins and external calls. Missing legacy counters also resembled measured zero. The disclosure now projects the existing resolution-rate counters, excludes explained sites, labels the remaining counts repository-wide, and treats absent/inconsistent metadata as unknown. | `devmap-analyze/src/model.rs` disclosure tests; `devmap-query/src/engine.rs` attribution tests; `incomplete_answers_say_so.rs`; malformed analysis tests in `devmap-store`. |
| Lost qualifications | Dependencies, semantic search, scoped trace, in-memory traversal, and composed graph layers could lose parse/attribution caveats. Coverage now follows each independently consumable result, including missing-path and unmatched-target results; retry warnings append to existing warnings. | 32 cases in `incomplete_answers_say_so.rs`, including clean controls, parse loss, known built-ins, unknown callbacks, depth/output limits, and found/missing paths. |
| Mixed generations | Dependencies could pair one generation's parse state with another's edges; semantic/explore queries could mix rows, counts, roots, and analysis. Store reads now pin the related data in one SQLite snapshot. Scoped traces use one generation index. Composed Explore/neighbors retry once and qualify every part if rebuilding crosses the second read too. | `paired_reads_are_one_generation.rs`: 100 alternating writes with pruning and 300 read attempts per new stress case; deterministic tests of stable, one-change, and persistent-change composition. |
| Collapsed method identities | Explore rebuilt IDs from bare names, collapsing same-named methods within one file. It now retains the full stored qualified identity and uses the legacy fallback only when that identity is absent. | Distinct same-named methods retain separate IDs and callers. |
| Ranking and partial radii | Explore ranked a narrow page, letting decoys hide an exact match, and did not disclose incomplete ranking or graph radii derived from omitted definitions. Search and Explore now share ranking and sampling disclosure, with definition materialization bounded by budget. | `search_ranks_before_paging.rs`: exact match among 200 decoys and tiny-budget sampling; omitted-definition radius checks. |
| Empty success after refusal | Missing stores could yield unavailable definitions/tests alongside available empty layers. Both halves now retain refusal semantics. | Empty-store Explore and affected-test checks plus GitPulse adapter regressions. |
| Memory/store boundary drift | In-memory queries accepted NaN thresholds and exceeded the stored engine's depth ceiling. They now reject NaN and enforce the existing 64-level maximum. | NaN checks and deep-chain tests for both traversal directions. Infinities retain the existing documented comparison behavior. |
| Confidence-filtered reachability | In-memory traversal crossed excluded low-confidence bridge edges and then emitted high-confidence edges beyond them. Confidence now participates in the shared graph index's edge-admission decision before traversal. | Forward and reverse bridge regressions; mutation checks on edge admission. |
| GitPulse metadata loss | `from_engine` discarded coverage/rung metadata for unavailable answers. Affected-test aggregation discarded real totals, truncation, layer availability, and layer warnings. The adapter now preserves each envelope, shares the caller's budget across batches, discloses repeated occurrences and independent snapshots across batches, and fails closed for incomplete selection. Existing whole-tree freshness refusal remains intact. | Runtime adapter and aggregation regressions; budget/overlap and mixed-failure metadata cases; 29 code-intelligence tests and 21 local-CI tests. |

The initial six added query cases all failed their targeted assertions while the 22 existing cases passed. Additional pre-fix runs reproduced two memory-boundary failures, the confidence-bridge failure, the ranking failure, and both host metadata defects. The host aggregation test specifically observed `total = 0` where the real engine reported one omitted affected test. These were runtime failures, not merely assertions that a new symbol should exist.

Mutation testing initially caught 23 of 25 selected mutations. It missed removal of the layered-query budget split and inversion of the retry-attempt guard. Stronger tests now assert combined token expenditure and exact retry/qualification behavior. The follow-up tested three selected mutations, including both prior misses, and caught all three. The third was a repeated fixture mutation whose source location moved. This was targeted mutation testing, not a workspace-wide mutation score.

## Verification

Run counts below overlap and must not be added as unique coverage.

| Check | Result |
|---|---|
| Baseline full `verify.sh` | 2,348 workspace tests passed; mandatory format, strict Clippy, determinism, performance, memory-model, growth, and incremental-equivalence gates passed. |
| Full pipeline after production repairs | 2,360 workspace tests passed; all mandatory gates passed. Later changes strengthened tests and completed the host-only adapter aggregation. |
| Final exact Rust tree | `verify.sh --quick` passed: 2,363 tests, formatting, and strict workspace/all-target Clippy. This final run occurred after the additional mutation guards, with release and host compilation completed. |
| Feature-off embedder matrix | 637 tests passed across extract, resolve, analyze, store, and query, checked independently with `--no-default-features --all-targets`. |
| Python DevMap unit seams | 174 passed. Four binary-selection tests initially failed because the explicit candidate override bypassed their mock setup; removing that override from the unit invocation restored the intended test configuration. |
| Python integration stress | 8 passed using the exact release candidate, including concurrent builds, interrupted persistence, hostile trees, large change bursts, and daemon fallback. |
| Capacity harness contracts | 7 passed. |
| GitPulse native consumer | 29 code-intelligence tests, 21 local-CI tests, and 10 embedding/link/real-store integration tests passed, including the normally ignored candidate-CLI test. Strict all-target Clippy passed; existing vendored macOS framework warnings remain. |
| Claude plugin validator | The normally ignored external CLI validator was explicitly run: 1 passed in 3.92 seconds. |
| Vendor/schema checks | All nine available source-crate comparisons matched, with no edited vendor files. Vendored and installed CLI schema both 20. Twelve framework upstream checkouts were unavailable; their local checksum checks passed, but upstream comparison was not claimed. |

The default ignored storm test was run separately. `concurrent_prune_writer_child` is a subprocess helper exercised by its parent rather than a missing standalone test. The optional mutation stage in `verify.sh` was run as the separate targeted campaign above.

One final quick run under simultaneous release/host compilation failed the existing Git churn timing assertion: `a flood past the cap must not cost the deadline: 3.376366459s`, against an unchanged 3-second bound. Its computed/truncated/content assertions passed. The complete five-test subprocess-bound target then passed. Ten focused package repetitions and ten repetitions of the exact workspace test binary passed; the latter measured 0.97–1.06 seconds each. Scheduling contention is an inference, not a proven root cause. No production code or threshold was changed in response to that isolated timing result. The complete final quick pipeline then passed on the unchanged Rust tree, including that timing assertion; its log is `/tmp/devmap-hardening-quick-isolated-final.log`.

## Stress results

- **128 worktrees:** 128 simultaneous daemons, 256 editors, 12,288 edits, 1,024 IPC queries, 512 metadata checks, 128 process kills/restarts, and 128 final cold-build comparisons passed in 113.146 seconds. Each worktree had 65 initial source files and a distinct private HEAD. IPC latency was p50 33.665 ms, p95 174.082 ms, maximum 375.640 ms. Aggregate daemon RSS samples decreased from 3,247,568 to 3,143,600 KiB. These are fixture and loaded-machine measurements, not production service-level guarantees.
- **40 filesystem storm cycles:** 10,000 creations, whole-directory renames, deletion/recreation with new content, and 1,000 chained rename cycles converged after kill/restart. Passed in 307.52 seconds; RSS half-means after warm-up were 127,407 to 130,288 KiB (+2.3%).
- **100 rebuild cycles:** restored graph digests stayed identical; RSS half-means were 25,582,951 to 25,493,072 bytes, and store half-means 1,469,689 to 1,469,817 bytes. Existing 10% plateau limits were unchanged.
- **1,200 daemon cycles:** passed actual final digest comparison, stable freshness through debounce, and stopped-state freshness. Peak RSS was 29,474,816 bytes. RSS half-means were 28,713,775 to 29,029,600 bytes; combined database/WAL half-means were 14,796,181 to 15,967,955 bytes, within the unchanged 10% gate. Maintenance reclaimed combined storage from 18,686,320 to 1,441,792 bytes near cycle 650; final stopped database was 1,671,168 bytes.
- **Self-build:** 1,672 files in 9.679 seconds under concurrent verification load; 172 MiB cold database and 751 MiB peak RSS, below the existing 10-second, 261-MiB database, 908-MiB scaled RSS, and 2-GiB absolute RSS gates. Baseline was 3.860 seconds on the same source family; the unequal machine load prevents interpreting that difference as a measured code regression.
- **Ambiguity model:** 5,000 sites with 100 then 200 candidates, 500,000 candidates weighed in the narrow case and 80,000 emitted edges. Measured cost was 110% of prediction; per-candidate cost changed to 98% when width doubled, within unchanged bounds. This synthetic probe does not measure an arbitrary repository's absolute peak.

## Live behavior and limits

The installed CLI's read-only `explore TaskBoard --json` probe in the active GitPulse worktree reported 20 shown definitions of 38 matches. Its radius preserved all three independent caveats: depth-3 truncation, repository-wide unresolved attribution, and callers potentially hidden by the 18 omitted definitions. At that observation, 78,374 of 162,472 unresolved sites lacked an indexed target after excluding 84,098 explained sites. These are repository-wide, time-specific counts, not the number of missing TaskBoard edges. Status still reported `is_fresh: false` with zero pending paths; the active worktree's graph was not represented as current or complete.

The repairs preserve fail-closed decisions, output/depth caps, bounded retries, snapshot isolation, cancellation checks, deterministic ordering, and explicit source freshness. They do not fabricate graph edges to remove a partial-result warning. Existing extraction limits, unsupported grammars, callbacks, unknown receiver dispatch, reflection, monkey-patching, and compiler/runtime semantics can still leave a static map incomplete. A rebuild addresses stale input; a larger budget addresses omitted output; neither proves runtime semantic completeness.

Native validation ran on macOS 27 / Apple Silicon with Unix-domain IPC. Linux/Windows execution, physical GitPulse UI delivery, real power-loss recovery, multi-week operation, and exhaustive mutation or all-language compiler equivalence remain unverified. The in-process GitPulse configuration cannot perform the parser-backed whole-tree freshness check; affected-test selection therefore continues to refuse narrowing when freshness cannot be established. This guard was deliberately preserved and tested.

## Reproduction and retained evidence

From `rust-port/`:

```sh
./verify.sh
cargo test --locked -p devmap-query --test incomplete_answers_say_so --test paired_reads_are_one_generation --test search_ranks_before_paging
DEVMAP_SOAK_CYCLES=40 cargo test --locked -p devmap-cli --test daemon_storm_soak -- --ignored --nocapture
python3 -m unittest discover -s tools -p test_worktree_stress.py
python3 tools/worktree_stress.py --binary /absolute/path/to/devmap --worktrees 128 --fixture-files 64 --rounds 4 --build-workers 8 --output /tmp/capacity.json
DEVMAP_BIN=/absolute/path/to/devmap SOAK_CSV=/tmp/build.csv tools/soak.sh /scratch/corpus 100
DEVMAP_BIN=/absolute/path/to/devmap SOAK_CSV=/tmp/daemon.csv tools/soak.sh /scratch/corpus 1200 --daemon
```

Run feature-off checks per crate; workspace feature unification can hide an embedder-only compile failure. Set `DEVMAP_BINARY` for the real Python stress suite and unset it for binary-selection unit fixtures. Use the repository's `scripts/vendor-crates.mjs` to update GitPulse copies rather than patching them independently.

Local raw evidence is in `/tmp/devmap-hardening-*.log`, with capacity results in `/tmp/devmap-hardening-capacity-final.json`, soak samples in `/tmp/devmap-hardening-*-soak.csv`, and mutation outcomes in `/tmp/devmap-hardening-mutations/mutants.out/`. Durable source regressions remain reproducible without these temporary logs. The [8 September audit](RELIABILITY_AUDIT_2026-09-08.md) records the earlier binding, source-read, and soak-harness repairs; its platform and semantic claims are historical, not substitutes for this run.
