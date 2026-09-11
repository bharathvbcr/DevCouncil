# Schema 20 qualification and coordinated cutover

Qualification date: 2026-09-09. **The implementation and staged artifacts are qualified within the measured workloads below; the installed consumer cutover is pending.** This report supersedes the earlier open native-platform and self-build findings in the [worktree design](AGENTIC_WORKTREE_DESIGN.md) and [staleness audit](STALENESS_AUDIT_2026-09-08.md). Historical failures remain evidence, not current success claims.

## What agents can rely on

**Verified:** ongoing edits do not disable navigation of a committed generation. Stale symbols and relationships remain queryable; changed-source snippets are withheld; new symbols require refresh. The exact staged GitPulse MCP returned all 28 matching symbols from a migrated real DevCouncil snapshot and withheld all 28 stale snippets. Its impact response explicitly reported 80 of 4,999 results, truncation and an incomplete walk. Requesting the GitPulse root against that DevCouncil store was refused.

**Inferred constraint:** no index can guarantee that a file remains unchanged after a query returns. Agents must consume freshness, generation, coverage and truncation fields and verify current source before edits. Missing graph edges, unsupported language constructs, or a capped walk cannot certify absence of callers or affected tests.

Each canonical worktree owns its mutable store, durable revision queue, writer lock and daemon endpoint. Concurrent worktrees share implementation and may share immutable artifacts; they do not share a mutable database. Navigation opens stores read-only. `devmap repair --schema` is the dedicated migration command. Writable store openers, including build and daemon startup, retain automatic migration of supported legacy schemas; they must also remain stopped until the coordinated cutover. Read-only navigation never migrates.

## Qualified source and artifacts

| Component | Exact identity |
|---|---|
| DevCouncil runtime | `da7d2d106201073e14d7ab43cc2f78981f6755ed` |
| Final qualification harness / CI | `da7d2d106201073e14d7ab43cc2f78981f6755ed`; includes the final extraction publication fix |
| GitPulse runtime and scoped vendor copies | `7b96c278098e583a9477f33db7871f1a8811265c` |
| Staged macOS CLI SHA-256 | `05df9a854a20b10dfc0bf082bd4cb05aa0af237208b768c99b758ee0d79ca423` |
| Staged GitPulse executable SHA-256 | `6e68710452b875d3721e033148f3ab0ad82cd4aff9a8ce61a749fa96a2ae061e` |
| Staged GitPulse MCP SHA-256 | `8a0674cf85d038464b00f76b4abc716f5032a82f3bd1bb8d5ca81cb02708c2a0` |

The CLI and application were built in separate qualification build directories. The app and all four Mach-O executables passed strict, deep ad-hoc signature verification. **These are local ad-hoc artifacts, not Developer ID signed or notarized distribution artifacts.** Installed versions remain unchanged. No release/tag/main merge was performed; qualification commits are on dedicated `codex/` branches.

The local artifact root is `/Users/bharath/Library/Application Support/DevMap/rollouts/2026-09-09-schema20/`. Only its `final/` directory contains the current candidates; earlier candidate directories and failures are retained for provenance. [AGENTIC_WORKTREE_QUALIFICATION.json](AGENTIC_WORKTREE_QUALIFICATION.json) records measurements and evidence hashes.

## Failures that drove the changes

The original audit reproduced cross-worktree store replacement, timestamp acknowledgement ABA, full/narrowed builds consuming uncovered events, repair racing re-enqueue, base-generation reads before writer admission, non-atomic migration, foreign-root reads, missed linked Git metadata and ignore changes, nested-root resolution, malformed queue identity, and database aliases bypassing writer ownership. Retained regression locations and invariants are in the [design](AGENTIC_WORKTREE_DESIGN.md).

The follow-up also reproduced and corrected these boundaries:

| Failure / evidence | Canonical correction and retained check |
|---|---|
| CLI navigation used a writable opener and migrated a legacy store | `devmap-cli/src/main.rs::open_for_read` uses the read-only opener. Persisted CLI tests prove navigation leaves legacy bytes unchanged; explicit schema repair validates arguments before opening and owns the writer lock. |
| Native Windows accepted hardlinked databases (`hardlink writer was accepted` in the pre-fix CI) | `devmap-store/src/db.rs::validate_database_file` checks native link count on Unix and Windows before readers, writers or lock acquisition. Tests refuse both names without changing bytes or creating lock files, then reopen after alias removal. |
| Windows `CTRL_BREAK` killed the daemon with status 3221225786 | Existing `ShutdownSignals` now handles the Windows break event. Native smoke requires successful graceful exit and endpoint retirement. |
| Windows fixture cleanup found SQLite handles still open | The Python stress harness closes inspection connections explicitly; transaction context exit alone does not close Python SQLite connections. Cleanup failure still fails qualification. |
| Windows atomic editor replacement raised `WinError 5` | Simulated editors retry only Windows errors 5/32/33 with a five-second deadline. Four harness tests enforce classification, bounded exhaustion and propagation. Receipts count retries separately; completed operations and cold equality remain mandatory. The exact process holding the conflicting handle was not identified. |
| MCP advertised a string reason while successful responses returned null; array type declarations bypassed validation | The existing canonical `mcp/mod.rs` and `mcp/validate.rs` correction was brought into qualification. Three tests failed pre-fix (`expected string, got null` and two invalid-type acceptance cases); all 113 MCP tests pass after correction. Exact-binary readback now checks all five navigation schemas and six response envelopes. |
| GitPulse watch retirement returned before backend callbacks released Windows directory handles | Watch sessions signal and acknowledge backend retirement outside the registry mutex with bounded deadlines. Alias cleanup, missing receipts and concurrent retirement have retained tests. |
| Ancestor queries repeatedly walked from the parse root | A scoped, bounded parent index replaces repeated walks. The pre-fix 500-function regression recorded 57,000 native root walks; the fixed test requires fewer than 500. Tree identity, nested scopes, unwinding, eight parsing threads, cap fallback and forced deadline refusal are tested. |
| Generation builds wrote disposable extraction payloads and then wrote them again during publication | Generation cache misses publish with the graph transaction; standalone cache callers keep their existing contract. A refusing SQLite trigger fails pre-fix extraction and passes the new path; the next committed build must hit cache. |
| Final scope collection or cleanup could cross the deadline after the last check and publish `Clean` | The public extraction boundary checks the clock and incomplete-walk latch after parser, tree and parent-index cleanup. Two real-parser fault-injection tests failed pre-fix (`late finalization published Clean`) and pass now. The full extraction crate passed 590 tests. Caller elapsed-time assertions cannot identify the publication instant after thread descheduling; deterministic internal deadline/latch tests replace that oracle while retaining outer wall-time and complete-graph assertions. |
| Native stream, file and cancellation behavior diverged | Retained regressions cover small native stack limits, broken pipe vs other output errors, bounded overload response draining, source size boundaries, file publication sharing, writer admission, cancelled drains and process-death recovery. |

The final extraction correction adds 93 lines and removes 6 across two source/test files. The preceding MCP-contract and probe-admission deltas add 160 lines and remove 12 across four files, including regression tests. Earlier audit changes are recorded separately; these counts exclude unrelated shared-checkout work. No dependency was added. Windows link inspection uses the existing system Kernel32 API. Filesystem integrity and root-selection checks do not widen authentication, authorization, permissions or access scope. The parent index retains one documented tree-lifetime-dependent unsafe conversion and bounds its ownership explicitly; passing tests are not a formal proof of unsafe-code soundness.

## Capacity envelopes

All runs use real detached Git worktrees and real daemons. Each worktree has two simultaneously released editors. Edits include replacement, rename, creation and deletion. Shared excludes and private HEAD changes must converge without a second source edit. Forced process death/restart is followed by recovery. Every final incremental graph must equal a separate cold build, including edge resolution/confidence and symbol spans; SQLite integrity and foreign keys must pass.

### Exact staged release on macOS 27 ARM64

| Measurement | 128-worktree workload | Larger graph workload |
|---|---:|---:|
| Daemons / editors | 128 / 256 | 8 / 16 |
| Source files per initial tree | 257 | 4,097 |
| Symbols / edges per initial tree | 2,307 / 3,843 | 69,635 / 126,979 |
| Rounds / file operations | 8 / 24,576 | 6 / 1,152 |
| Successful IPC requests | 2,048 | 96 |
| Metadata checks | 512 | 32 |
| Forced kills and restarts | 256 | 12 |
| Cold graph comparisons | 128 / 128 | 8 / 8 |
| Elapsed seconds | 116.181 | 96.785 |
| IPC p50 / p95 / max, ms | 33.437 / 155.500 / 575.974 | 8.998 / 19.746 / 32.983 |
| Peak sampled aggregate daemon RSS, KiB | 4,943,504 | 3,832,480 |
| Concurrent build/check workers | 16 | 8 |

Both runs used the exact CLI SHA above, two Tokio and two Rayon workers per process, local storage and a 64 GiB host. The 128-worktree envelope requires those launch settings; default thread-pool sizing was not qualified at that concurrency. Configure the launcher environment explicitly for that deployment profile. Latency measures direct Unix-socket requests, excluding probe-process startup and MCP/agent overhead. RSS is sampled aggregate daemon resident memory, not an instantaneous peak of all child processes or a memory quota.

To reproduce the release capacity profiles, build the CLI, set `CANDIDATE` to its absolute path, and run from `rust-port/`:

```sh
cargo run -p devmap-cli --example worktree_stress -- --binary "$CANDIDATE" --worktrees 128 --fixture-files 256 --rounds 8 --build-workers 16 --output capacity-128.json
cargo run -p devmap-cli --example worktree_stress -- --binary "$CANDIDATE" --worktrees 8 --fixture-files 4096 --functions-per-file 16 --rounds 6 --build-workers 8 --output capacity-large.json
```

The harness sets `TOKIO_WORKER_THREADS=2` and `RAYON_NUM_THREADS=2` for its child processes. A deployed launcher must carry the same settings to claim this 128-daemon envelope. The native workflow additionally passes its built `ipc_probe`; `--probe-workers` defaults to eight. The existing `verify.sh` remains the required repository performance and correctness gate.

### Native hosted CI

Native capacity is run by the committed workflow on Linux x86_64, macOS ARM64 and Windows x86_64, after the full native Rust suite. Each job runs a two-worktree graceful-shutdown smoke followed by 128 daemons / 256 editors, 65 initial source files per tree, four rounds, 12,288 file operations, 1,024 IPC requests, 512 metadata checks, 128 forced restarts and 128 complete cold comparisons. Build/check concurrency is eight; per-process Tokio/Rayon workers are two. Native receipts include the platform and binary hash.

Unlike the local release measurement, native CI uses a bounded subprocess IPC probe for Unix sockets and Windows named pipes. Its latency includes process startup and must not be compared directly with the direct-socket measurements. Final native results are recorded in the qualification JSON. A prior Windows run failed after three rounds because 256 measurement-process launches exhausted process-creation resources (`WinError 8`); its receipt records 9,228 completed operations and zero completed cold comparisons. The harness now bounds probe processes to eight independently of the 128 daemons and 256 editors, with a 30-second admission deadline, peak concurrency reporting, and latency including admission wait. Process creation errors are not retried into success. A subsequent Windows run failed in Git while writing a shared commit object during metadata-only edits. All simulated commits had identical metadata; the new distinct-HEAD assertion reproduced that gap locally. Each commit now includes its worktree identity, and the harness requires every private HEAD to differ. The two-tree red/green receipts are retained. That run passed all 128 Windows cold comparisons but failed the finalization assertion described above. The final runtime rerun passed all six CI jobs, including every native suite and all capacity checks.

Final native capacity results for runtime `da7d2d10`:

| Measurement | Linux x86_64 | macOS ARM64 | Windows x86_64 |
|---|---:|---:|---:|
| Full Rust suite passed / failed / explicit skips | 2,335 / 0 / 3 | 2,335 / 0 / 3 | 2,234 / 0 / 3 |
| Complete cold comparisons / distinct private HEADs | 128 / 128 | 128 / 128 | 128 / 128 |
| Capacity elapsed seconds | 125.342 | 118.195 | 262.390 |
| Probe p50 / p95 / max, ms | 337.676 / 814.012 / 1114.656 | 805.352 / 1115.873 / 1584.029 | 7218 / 12313 / 13266 |
| Peak sampled daemon RSS, KiB | 4,623,548 | 3,799,472 | 3,276,388 |
| Filesystem retry attempts | 0 | 0 | 1,110 |

Every native suite produced 219 result blocks; platform-specific tests explain differing totals. Windows sharing retries remained inside the five-second per-operation deadline. Native probe latency includes bounded admission and process startup and is not an agent navigation SLA. No failed run was counted as complete cold-build coverage.

## Performance and suites

[DevCouncil qualification CI](https://github.com/bharathvbcr/DevCouncil/actions/runs/34344358354) passed the unchanged full `verify.sh` required gates. The final self-build took **8,599 ms** for 1,647 files, with 170 MiB cold database size against 257 MiB budget and 681 MiB peak RSS against the 894 MiB scaled budget. Deterministic rebuild, ambiguity memory model, five-build storage plateau and five incremental/cold comparisons passed. The default workspace gate recorded 2,335 passed, zero failed and three explicit skips across 219 result blocks. Counts from separate runs are not additive unique-test coverage.

The exact staged release also built the 1,649-file qualification checkout in **3.268779 seconds** on the local Mac, peak RSS 776,617,984 bytes. The local checkout includes the uncommitted qualification documents; its file count differs from hosted CI. An earlier loaded Mac run failed the same ten-second gate at **17,386 ms**, despite its 2,333 tests passing. That failure is retained; passing CI and the isolated measurement close the measured gate, not an unconditional latency SLA. The required threshold was never raised. The optional `cargo-mutants` phase was not run; explicit pre-fix failures and fault injection do not establish workspace-wide mutation coverage.

[GitPulse final CI](https://github.com/bharathvbcr/GitPulse/actions/runs/34344431921) passed on all three native platforms. Rust output totals were macOS 2,071, Linux 2,070 and Windows 1,916 passed, zero failed, with ten explicit skips per platform across 61 result blocks. Platform-specific collection explains differing totals. These include the library and integration targets, not just code intelligence. The skips are manual document timing, live upstream update, opt-in candidate CLI integration, deep graph fuzzing, one real Pulse repository test and five real history/status smoke tests. The candidate CLI integration passed separately locally; unrelated upstream/history/deep-fuzz checks are not claimed as executed by this audit.

The DevCouncil default skips are the external Claude plugin validator, a minutes-long daemon storm and a subprocess helper invoked by its parent. The earlier staleness audit explicitly ran the validator and a 40-cycle storm against its recorded prior binary. This qualification instead runs the new multi-worktree process-death workloads against the final source/artifact; it does not relabel the older storm as a final-binary run.

## Migration and readback

**Verified on copies of all 16 inventoried live stores:** schemas 17, 18 and 19 migrate to 20 with the exact staged CLI; integrity and foreign keys pass; original row multiplicity and payload fields are preserved; repeating repair preserves epoch/revision identity; real-symbol search succeeds; old CLI readers explicitly refuse schema 20; retained backups reproduce their recorded hashes for rollback. Navigation against each legacy copy is byte-immutable before explicit repair.

Schema 17's existing migration remaps internal edge/unresolved ordinals into global row IDs. The rehearsal compares all other original fields and row counts and reports those two ordinal remappings explicitly. It does not misclassify intentional internal ID changes as lost graph payload, or silently exempt any other field.

During diagnosis one live DevCouncil store was inadvertently migrated through the old writable navigation path. It was restored under the writer lock and an exclusive SQLite transaction after comparing all 21 original table payloads to the retained backup. Only schema-20 additive fields/tables were removed; generation 1,276 and graph rows were preserved. Restoration receipts are retained. The last read-only live inventory verified all 16 stores still at their original schema 17/18/19 versions. Other active tasks may advance their generations; those updates must be preserved at cutover.

The final signed MCP passed checks of all five advertised navigation schemas and six actual response envelopes, including null freshness/reason values. Its real-store readback proves initialize, tool discovery, search, stale-snippet withholding, bounded impact with honest incompleteness, foreign-root refusal and clean process exit. The official MCP doctor reports schema 20 and status OK. The official vendor/schema checker passes with the staged CLI and correctly rejects the installed schema-19 CLI beside schema-20 source.

## Coordinated cutover: pending live operation

1. Stop/disconnect GitPulse MCP in Codex and Claude, and close the GitPulse application. Verify old CLI/daemon/MCP processes and live database handles have retired. Keep new build/serve/autospawn writers stopped too; writable openers can migrate supported legacy schemas. Do not replace or kill another active session underneath its work.
2. Inventory live stores again and take fresh online SQLite backups plus installed-binary backups. Validate integrity, foreign keys, generations, owners and hashes. The initial rehearsal snapshots are not a substitute for backups of the latest live generations.
3. Install the staged CLI/daemon and the matching GitPulse application/MCP binaries together. Verify exact executable paths, schema versions and candidate hashes before any client restart. All consumers in this installation must agree on schema 20.
4. Run the new CLI's explicit `devmap --db /absolute/path/to/devmap.sqlite repair --schema --json` for each inventoried store while clients remain stopped. Migration owns the existing writer lock; a busy/unknown writer is a failure, not a reason to bypass admission. Validate original payload preservation, new schema, queue identity, integrity and foreign keys for each store.
5. Restart/reconnect supported clients, verify running executable identity, and check real-root status, search, impact, stale-snippet behavior, one edit/rebuild cycle and daemon recovery. Until this step runs, installed end-to-end navigation remains unverified.
6. If migration/readback fails, keep clients stopped and restore the fresh database and matching binary set together. Never run old readers against schema 20 or downgrade by editing only the schema stamp.

The available Computer Use tool refused control of `com.openai.chat`; no private socket, app scripting or process-kill workaround was used. A user-supported stop/reconnect is required to coordinate those active MCP sessions. Build, backup, migration rehearsal, native CI and exact-artifact readback have been completed independently of that client lifecycle gate.

## Explicit limits

The measured workload supports hundreds of simultaneous editors across distinct worktrees on the tested local filesystems. It does not prove hundreds of large monorepos on one host, all language/framework semantics, every OS/architecture/filesystem, network filesystem lock correctness, power-loss durability, multi-week production stability, or a universal latency/memory SLA. Per-daemon admission bounds remain enforced; there is no newly introduced global memory/disk quota. The larger graph workload is synthetic and separate from native small-worktree capacity. Native CI qualification and ad-hoc Mac packaging do not constitute Windows/Linux release installation or trusted platform publication.

Native qualification is complete for both repositories. The remaining deployment gate is coordinated live cutover and installed readback above. The rebuilt and signed MCP candidate has passed exact-artifact contract readback. Broader unmeasured capacity and correctness claims remain explicitly out of evidence; they are not converted to success by repeating a bounded test.
