# DevMap staleness and navigation audit — 8 September 2026

## Verified behavior and repair

Staleness does not disable persisted navigation. A quiet watcher queue is not proof of current source. Real CLI tests reproduce `is_fresh: false`, `pending_count: 0`, and `query_ready: true` after a same-size edit. Stored identities remain searchable, changed-source snippets are withheld with a reason, and newly added identities appear after rebuilding. Unchanged files still return hash-verified snippets while another file is stale.

`StoreStatus::is_fresh()` and `StoreStatus::freshness_reason()` in `crates/devmap-store/src/db.rs` now own the shared verdict. The existing public helpers in `crates/devmap-serve/src/protocol.rs` delegate to them. This preserves the CLI and daemon policy and lets GitPulse's embedded reader use it without a duplicate policy or a dependency on the daemon. Its generated vendor copy was refreshed using GitPulse's `scripts/vendor-crates.mjs --crate=devmap-store`.

GitPulse's related adapter repairs preserve query freshness and snippet omissions, expose status uncertainty to agents, refuse affected-test certification on failed status reads, and retry a dirty event skipped by an occupied writer. Details and client regressions are in that repository's `docs/DEVMAP_STALENESS_AUDIT.md`.

## Verified checks

| Check | Result and evidence |
| --- | --- |
| Unmodified store/query/serve baseline | 922 passed, 0 failed, 1 ignored; `/tmp/devmap-staleness-baseline.log`. |
| Format and strict workspace/all-target Clippy | Passed in `verify.sh`; `/tmp/devmap-staleness-verify-final.log`. |
| Complete default workspace tests | 2,289 passed, 0 failed, 3 ignored, across 218 test targets including doc tests; same log. |
| Real CLI staleness regressions | 20 same-size edit/rebuild cycles across search/deps/impact/trace/dead/explore; add/rename/delete/restore and unchanged-file snippet checks. `crates/devmap-cli/tests/test_persisted_cli.rs`, all 3 tests passed. |
| Deterministic cold rebuild | Both graph digests equal `6fd5be321abc9f1183a8b77bade5a98a529a1a8d345a3a7c51410ba1e3128c26`; same verify log. |
| Daemon storm, explicitly enabling the ignored test | 40 cycles passed in 367.47 seconds. Cycles rotate through 10,000 creations, mass directory rename, delete/recreate, and 1,000 chained renames, with kill/restart and cold-build equality assertions. RSS half-means after 10 warm-up cycles: 117,875 to 119,700 KiB (+1.5%). `/tmp/devmap-staleness-pinned-storm40.log`. |
| Memory-model gate | Passed: 108,374 milli-bytes per candidate against 150,000 cap; doubling candidate width gave 98% of the initial rate, against 125% cap; measured fan-out cost 111% of prediction. Synthetic, capped corpus: 200 files, 5,000 ambiguous sites, 80,000 emitted edges, 500,000 candidates. `/tmp/devmap-staleness-remaining-gates.log`. |
| Five-build storage plateau | Passed: 1,163,264 / 1,359,872 / 1,425,408 / 1,392,640 / 1,409,024 bytes; 2 retained generations. Same log. |
| Incremental/cold smoke gate | Passed all 5 cycles; this smoke test explicitly does not assert long-run storage growth. Same log. |
| External plugin validator | `the_emitted_bundle_passes_claude_plugin_validate_strict` explicitly enabled and passed in 3.58 seconds; `/tmp/devmap-staleness-plugin-validation.log`. |

The default suite's remaining ignored `concurrent_prune_writer_child` is a subprocess helper exercised by its parent. The concurrent new `documentation_visibility` integration target was verified with parsers disabled and registered as feature-off-safe so the existing classification gate could run without being weakened.

## Failed runs and verification limits

The complete `verify.sh` invocation is **not an all-green result**: its self-build time gate failed at 15,331 ms, then failed at 10,957 ms on a lighter-load replay, against the unchanged 10,000 ms threshold. The replay used 1,637 source files, a 166 MiB cold database against 255 MiB budget, and 737 MiB peak RSS against the stricter 887 MiB scaled budget. Those size/memory comparisons passed. Stages 6–8 were then replayed separately, using their exact project-script bodies and unchanged thresholds. Shared-host load is a possible contributor to timing; it is not proven to be the sole cause. No performance threshold was raised.

After this audit's native CI ended, a final replay took 11,522 ms over 1,638 files, with 166 MiB database size and 724 MiB peak RSS against an 888 MiB scaled budget. Other Cargo work was still active, so this was not a quiet-host measurement despite the log's filename (`/tmp/devmap-staleness-performance-quiet.log`). Latency remains an open gate; no further retries were used to seek an arbitrary green result.

Earlier attempts hit `No space left on device`. Only checked, untracked, unused Cargo incremental caches were removed; source, data, models, and active build products were preserved. Another concurrent task later removed the shared Cargo binary while a stress run was executing. That interrupted run is not counted as completed coverage. The successful storm uses an unchanged repository test compiled against a pinned release executable outside the shared target directory:

- Candidate SHA-256: `f38f4047bdf59d928a51338ebd105f3d9af9fdc4c8fe7b3b5a5db969d86c4d45`.
- Stress-test source SHA-256: `6254cf5704adf9d67339d3d9caac591cad96be02a99ee2b395ee4363d95b9d65`.
- Pinned test/executable: `/tmp/devmap-staleness-pinned/`.

GitPulse's parser-free reader cannot certify the compiled grammar identity and reports that uncertainty. A fresh CLI status is a point-in-time observation, not an atomic filesystem snapshot or a guarantee against the next edit. Query `source_freshness: null` explicitly means that whole-tree freshness was not checked. Capped or incomplete graph traversal cannot justify skipping tests.

Unverified: every operating system/filesystem, power-loss recovery, a multi-week production soak, full dynamic-language/static-analysis completeness, workspace-wide mutation coverage, and installed consumer cutover. These checks establish bounded local evidence on macOS, not universal correctness. Existing unrelated work and historical open items in `STATUS.md` remain separate.
