# DevMap for concurrent agent worktrees

Audit and implementation: 2026-09-09. Current qualification evidence is in [SCHEMA20_QUALIFICATION_2026-09-09.md](SCHEMA20_QUALIFICATION_2026-09-09.md); the initial local measurements below are historical. Canonical implementation lives in DevCouncil's Rust workspace; GitPulse embeds scoped copies of `devmap-store` and `devmap-extract`.

## Navigation contract

A map is a committed snapshot of **one worktree**, not a promise that all files still have those bytes. During edits, agents can navigate the last committed symbols and relationships. Source hashes gate snippets: changed source is withheld, and new symbols become discoverable after refresh. Agents must read generation, freshness, pending work, coverage, and truncation fields. An unavailable query cannot justify “no callers” or “no tests.” This distinction remains necessary even with flawless indexing: an editor can change a file immediately after a query returns.

Agents should pass an absolute repository root through the GitPulse MCP adapter. Rootless CLI queries resolve the nearest Git worktree root, including from nested directories; root-taking commands retain their explicit scope. An explicit `--db` can intentionally inspect a snapshot from another directory. Non-Git trees still use the supplied/current root.

## Architecture and invariants

| Boundary | Enforced contract | Owner |
|---|---|---|
| Worktree identity | One canonical root owns a store, even before its first generation. A shared absolute `DEVMAP_HOME` cannot silently overwrite another worktree. | `devmap-store`: `bind_repo_root`, generation transaction |
| Agent reads | CLI navigation and GitPulse readers use the read-only store opener, validate schema and requested root, and never migrate as a side effect. Explicit CLI database reads retain intentional cross-directory inspection. | CLI `open_for_read` / status; GitPulse `open_store` |
| Durable edit identity | Each event gets a monotonically increasing revision and a store epoch. Wall clocks are diagnostic only. Deletion and re-insertion cannot reuse an acknowledgement. | `pending_state`, `PendingClaim`, `PendingWatermark` |
| Acknowledgement | Clear or charge a retry only if the claimed revision still exists in the same store. A later edit resets its own retry history. | Store queue methods |
| Build coverage | A full build clears only revisions at or before its pre-discovery watermark. A narrowed build also requires path coverage. HEAD tokens and quarantined work obey those same rules. | `PendingSupersede` |
| Repair | Conditional deletion/renaming checks the observed revision. Merging spellings creates a new revision so old claims cannot consume the merged event. | `reconcile_pending_paths` |
| Database aliases | Symlinks share the canonical writer lock. On Unix and Windows, hardlinked databases are refused before opening or taking a lock; independent WAL paths cannot safely share an inode. | Store file boundary |
| Writers | Hold the per-store process lock before reading the base generation, through extraction, publication, pruning and acknowledgement. Other worktrees have independent locks. | CLI build and daemon drain |
| Migration | Explicit `devmap repair --schema` acquires writer ownership. One immediate SQLite transaction covers version inspection, migration, validation and the new stamp. Re-entry preserves the epoch and revision counter. Invalid repair arguments are refused before opening the store. | CLI repair; Store migration |
| Git metadata | Resolve `.git` directories, relative/absolute gitfiles, and `commondir`. Watch private HEAD and shared refs/excludes outside linked worktrees without recursively watching Git object storage. | `git_metadata`, watcher |
| Ignore changes | Rule changes enqueue a full rescan; changing which source is visible does not require a second source-file edit. | Watcher |
| Recovery | Pending events survive process death; OS locks release; restart reclaims stale endpoints and reconciles source against the saved generation. | Store, daemon, IPC |

Generation builds reuse extraction payloads from committed generations. Cache misses stay in memory until `save_generation` publishes them in the same transaction as the graph. They no longer write and immediately delete a second full copy through `extraction_cache`. An interrupted build may repeat uncommitted extraction work; it cannot acknowledge edits through a generation that was never published. Standalone extraction callers retain the existing immediate-cache API.

The parser builds a scoped parent index with at most 131,072 links and eight active trees per thread. Exact tree identity separates nested and concurrent parses; scope exit and unwinding retire the index. A cache miss uses the native parent lookup with the existing deadline checks. This removes repeated root walks without replacing timeout refusal with partial authoritative output. Completed hostile inputs must contain their full declaration set; forced-deadline tests require explicit refusal and no partial graph. Direct extraction also enforces the scanner's existing 1 MiB source limit. The public extraction boundary rechecks the deadline and incomplete-walk latch after final scope collection and parser/tree/index cleanup; late work cannot publish a clean graph.

Per-worktree mutable state stays independent. Share code and immutable artifacts deliberately; do not share a mutable SQLite database between roots. A global scheduler is not required for correctness. Capacity controls belong to the launch environment and the existing per-daemon admission limits.

## Defects reproduced before correction

1. Two linked worktrees using one absolute state directory both built successfully; querying the first returned the second worktree's symbol.
2. Equal timestamp re-enqueues were deleted by an old claim.
3. A full build discarded a later HEAD event; a narrowed build discarded quarantined work it never read.
4. Queue repair deleted an event enqueued after inspection.
5. The daemon read its base before obtaining writer ownership.
6. Concurrent legacy migration openers could overwrite a peer's newer schema stamp; the new additive migration exposed duplicate-column failures.
7. GitPulse accepted a store bound to another root.
8. Linked-worktree ignore checks failed with `Not a directory`; shared metadata was outside the watcher root.
9. Ignore-rule events cleared the cache but queued no work, so newly visible/hidden files were not reconciled.
10. Rootless queries from nested directories looked for a new local store instead of their worktree map.
11. A current-schema store with a missing queue identity opened successfully; joining claims against that missing row could disguise queued work as an empty batch. Both explicit read-only readers and the ordinary opener's filesystem read-only fallback now validate that identity. The fallback had its own failing regression before correction.
12. A symlinked database could acquire a different writer lock, and hardlinked database names were accepted despite distinct WAL/lock paths.

Surrounding regressions cover old-attempt retry accounting, backwards clocks, delete/reinsert ABA, cross-store claims, revision exhaustion rollback, canonical-spelling merges, migration re-entry, malformed/oversized Git pointers, and reader immutability.

Older expectations were updated to preserve the new invariants: task cancellation does not kill a blocking drain; unreadable roots report failure while preserving work; events are ordered by revision; and writer-admission failure charges no per-path retry. Real process-death tests remain separate.

## Capacity and verification

`tools/worktree_stress.py` creates real detached linked worktrees with the same initial HEAD and relative paths. It runs one daemon per worktree and two simultaneously released editors per worktree. Each round includes atomic replacement, rename, creation/deletion, navigation over IPC, forced process death/restart of one quarter of the daemons, and convergence checks. It also changes shared `info/exclude` without source edits and creates private HEAD-only commits. Finally, every incremental graph must equal a cold build, including symbol paths/spans and edge resolution/confidence; SQLite integrity and foreign keys are checked.

The harness reports actual daemon/editor concurrency separately from the bounded number of CLI build/check workers. It records the candidate binary hash, elapsed time, operation counts and aggregate daemon RSS samples. It uses disposable trees and cleans up only processes and files it created. Native CI uses the bounded `ipc_probe` example for both Unix sockets and Windows named pipes; its latency explicitly includes starting the probe process. SQLite inspection handles are closed explicitly. Orderly shutdown must exit successfully and release the endpoint; abnormal termination, missing measurements and cleanup failure cannot become a passing receipt.

### Initial local results (superseded candidate)

The frozen candidate completed the following macOS/local-filesystem workload. The machine-readable result is [AGENTIC_WORKTREE_VALIDATION.json](AGENTIC_WORKTREE_VALIDATION.json).

| Measurement | Observed result |
|---|---:|
| Simultaneous worktrees / daemons / editors | 128 / 128 / 256 |
| Rounds / source file operations | 12 / 36,864 |
| Navigation requests over IPC | 3,072 successful |
| Metadata-only checks | 512 successful |
| Forced daemon kills and restarts | 384 |
| Incremental graphs equal to cold rebuild | 128 of 128 |
| Elapsed wall time | 129.010 seconds |
| IPC latency p50 / p95 / maximum | 38.374 / 204.218 / 521.545 ms |
| Aggregate daemon RSS range across 12 samples | 3,767,056–3,788,384 KiB |

Reproduce from this workspace with `python3 tools/worktree_stress.py --binary /absolute/path/to/candidate/devmap --worktrees 128 --rounds 12 --build-workers 16 --output /absolute/path/to/result.json`. This measures daemon IPC round trips, not the complete agent or MCP interaction latency. The frozen load-test binary SHA-256 is `8170efd7d02fa345ae5d3c7f6057a5034dfeed50e7617d66860ba973fc677027`.

| Check | Result and scope |
|---|---|
| Rust workspace suite | 2,311 passed, 0 failed, 3 ignored, across 218 test-result blocks |
| Parser-free crate suites | 621 passed across extract, resolve, analyze, store and query; 0 failed, 1 ignored |
| Final read-only validation delta | 65 store reader/hardening tests passed; 16 reader tests also passed without default features |
| GitPulse library suite | 1,515 passed, 0 failed, 2 ignored; an additional subprocess assertion passed |
| Final GitPulse adapter delta | 25 code-intelligence tests passed, plus its subprocess assertion |
| Final GitPulse transport and embedding suites | 28 passed, 0 failed, 0 ignored, including the opt-in real candidate CLI integration |
| Static and integration consistency checks | Rust formatting and Clippy with warnings denied passed in both workspaces; all nine vendored crates matched their canonical sources; CLI and embedded schema both 20 |

Counts above are separate runs, not additive unique-test coverage. The full workspace and 128-worktree run preceded the last read-only fallback validation and a Clippy-only needless-borrow cleanup; the final delta was validated separately. The default kernel skips are the external Claude validator, the explicit long-running daemon soak, and a child-process helper invoked by its parent. This follow-up uses the separate 128-worktree crash workload. GitPulse's two library skips are an unrelated manual document-refresh benchmark and a live upstream-update check. Neither skipped external check is reported as passing.

The initial audit's final candidate (`f0463d910a61de9beadb669ce8348d40ceb2b71419352b3df9d45fb3742be03a`) also passed a four-worktree/eight-editor smoke run: 384 file operations, 32 IPC queries, 16 metadata checks, four forced restarts, and four matching cold comparisons. [AGENTIC_WORKTREE_FINAL_DELTA.json](AGENTIC_WORKTREE_FINAL_DELTA.json) retains that result separately from the capacity run.

Retained adversarial cases live in `devmap-store/src/db.rs` (`agentic_queue_regressions`), store `embedded_reader` and `migration_ladder` tests, CLI `test_concurrency`, watcher tests, and extract `ignore_rule_tolerance`. GitPulse's `codeintel` tests check foreign-root refusal; `devmap_embedding` drives the candidate CLI through its existing adapter. The tracked canonical store/extract/serve diff is 985 lines added and 283 removed, plus the new 78-line Git metadata resolver and 286-line stress harness. These scoped counts include the earlier staleness audit and exclude the shared CLI, consumer, and documentation changes. No dependency was added.

The source fixtures are deliberately small. Hundreds of simultaneous small worktrees do not establish capacity for hundreds of large monorepos. The measured configuration uses `TOKIO_WORKER_THREADS=2`, `RAYON_NUM_THREADS=2`, and 16 concurrent build/check workers. Per-daemon limits already include 64 IPC connections, 4,096 buffered watcher events, 4,096 debounce paths, and a 10-second maximum debounce hold; overflow requests a full rescan. Durable queue size remains dependent on outstanding distinct paths and disk capacity. There is no new global memory or disk quota in this change.

## Compatibility and rollout

Schema 20 is a deliberate compatibility boundary. Schema-19 binaries and embedding readers must not share an upgraded store. Ship the CLI, daemon and GitPulse reader together; stop old writers, retain a recoverable SQLite backup, migrate with the new writer, then verify reader schema agreement and navigation. Readers never migrate stores. Writable `Store::open` callers, including build and daemon startup, can migrate supported legacy schemas; keep those writers stopped until the coordinated cutover, then use the dedicated schema-repair command for controlled readback. The rollout audit exposed an older CLI navigation path that did migrate as a side effect; it was replaced with the read-only opener and a failing regression. One live DevCouncil store was inadvertently migrated during diagnosis, then restored to schema 19 under writer ownership after verifying all original table rows against its retained backup. The restoration preserved the published generation and graph. Live stores remain on their prior schemas while old clients are still connected.

A copied or moved store retains its owner. Use a fresh per-worktree store for a relocated checkout; silent rebinding is refused because the old checkout may still be active. Git metadata relocation while a watcher is running requires restarting it. Local filesystems are the validated deployment target. Native Linux, macOS and Windows qualification and its measured capacity envelopes are recorded in the follow-up report. Network filesystem locking semantics remain outside the validated target.

The follow-up closes the unchanged 10-second self-build gate with a final hosted Linux result of 8,599 ms, plus a 3.269-second frozen-release measurement on macOS. The earlier loaded-host failure is retained in the report. These results establish measured envelopes, not universal latency, memory, platform, or zero-bug guarantees. The coordinated installed-client cutover remains pending.

Git metadata semantics were checked against the official [repository layout](https://git-scm.com/docs/gitrepository-layout) and [worktree documentation](https://git-scm.com/docs/git-worktree).
