# One-file update profile audit — 2026-09-12

**Verified:** the clean release reproduces the prior one-file edit gap. Over seven alternating-order edits, DevMap's direct process median was **719.3 ms** (670.9–749.8 ms), and CodeGraph's was **314.4 ms** (298.0–338.4 ms). These are measured observations on one fixed repository and one edit shape, not population rankings. No latency optimization was selected from this result.

## Evidence and reproducibility

The DevMap executable was compiled from clean commit `ee07c183b127e7291c38f993f4d71ca75f1121dc` using `cargo build --release --locked -p devmap-cli`. Its SHA-256 is `cce843d25f48323c90bf92079ee2b7be79af5b1bd5a63c4661acc5aa53c8bddd`. The binary, clean status, compiler version and Cargo.lock digest are preserved in `.devcouncil/audit/baseline/`. This establishes provenance for this new run; it does not recover the missing patch of the previous dirty binary.

The runner is `.devcouncil/audit/update-profile/profile.py`. Its `measurements.jsonl` records every command, working directory, return code, direct process elapsed time and load average; each command's stdout/stderr is preserved beside it. `summary.json` contains all phase medians and ranges. The runner uses a 120-second process limit with a bounded TERM/KILL escalation, then checks successful exits and parseable output before accepting evidence.

Each tool received its own clean clone of the prior 1,186-file commit. All 1,186 source hashes in each clone matched the preserved benchmark manifest both before and after the run. The measured edit appends a new Python function to `benchmarks/tasks.py`, replacing the previous probe each round. Every inserted name was returned by the respective tool at the edited path; the final removed name was absent after restoration. There was one empty-index build per tool, seven edit repetitions, and three unchanged controls per tool. Providers were not enabled or installed. CodeGraph's installed `init --help` and `sync --help` were checked before invocation.

Unlike the earlier benchmark, this runner measures direct subprocess wall time without a BSD `time` wrapper or 50-ms process-tree RSS sampler. Its measurements must not be merged with the original 817.4/376.5-ms medians as if they used an identical protocol. OS caches were not flushed, machine background load was not controlled, and no confidence interval was computed. Cold control timings are single observations, not cold medians.

## Where the time goes

**Verified:** the preserved original `edit-devmap-{1,2,3}.stdout` already contained native phase timings. The old report's claim that no phase was identified overlooked this existing evidence. Those three records yield scan/extract 199.4 ms, resolve 220.8 ms, analysis 68.9 ms and persistence 255.4 ms medians. Inside persistence, unresolved-ledger writing was 118.0 ms and generation pruning 51.6 ms. Thus the earlier evidence supported phase-level triage, although it did not establish a safe optimization.

The new clean release records the following seven-edit medians:

| Phase | Median ms | Observed min–max ms |
|---|---:|---:|
| Scan and extract, including setup/index construction | 187.0 | 175.9–192.4 |
| Resolve all 857 files | 196.7 | 192.4–211.6 |
| Analyze 33,790 edges | 64.7 | 60.9–67.8 |
| Persist generation and reclaim | 226.4 | 176.8–229.8 |
| Native measured total | 668.4 | 621.5–696.0 |
| Entire process | 719.3 | 670.9–749.8 |

Nested persistence medians were 111.4 ms for unresolved rows, 34.0 ms for generation pruning and 7.5 ms for vacuum/checkpoint. Nested times are already included in their parent; they must not be added a second time. Medians from separate distributions are not additive. The first edit creates generation two without needing to prune an older retained generation, so its lower persistence latency is expected and retained in the reported range.

**Inferred:** costs are distributed across cached extraction, global resolution and persistence. Vacuum is not the dominant cost in this workload. A smaller competing directory does not establish that persistence writes unnecessary data, and CodeGraph's lower median does not identify which DevMap computation can safely be removed.

## Observability fix and preserved invariants

The scan/extract headline previously combined writer-lock waiting, store opening, queue reconciliation, discovery/read, previous-hash loading, cached extraction, affected-write-set computation and resolver indexing. Only the Go-module scan had its own nested duration. The CLI now brackets those operations with its existing `ProgressReporter::timed` owner:

`writer:wait`, `store:open`, `pending:reconcile`, `scan:read`, `scan:previous_hashes`, `extract:files`, `affected:closure`, and `resolver:index`.

This changes measurement granularity, not the graph or update algorithm. Failed operations retain their elapsed duration and original error. Skipped operations remain absent: unchanged builds report no extraction, closure or resolver indexing, and `--full` reports no affected-closure span. Child durations remain contained by the top-level stage and must not be double-counted. JSON progress-disabled mode still collects the profile.

Global resolution and analysis remain intact. Their callers require whole-repository symbol/type context and liveness; narrowing them merely because one source file changed would require an independently proved dependency/invalidation contract. The unchanged fast path, generation atomicity, writer lock ordering, cache identity checks, pending-watermark semantics, bounded retention and coverage disclosures remain in place.

Two executable CLI regressions were first run against the code without this instrumentation and failed: `update_profiles_separate_work_and_do_not_invent_skipped_phases` reported `missing writer:wait`, and `a_failed_store_open_is_timed_without_claiming_extraction_ran` reported absent timings for its failed store open. The first covers cold, one-file edit, unchanged and full builds, verifies only the work that ran is reported, bounds nested duration sums, and checks full/edit symbol and edge counts agree. The second corrupts a fixture store and ensures the failed open is timed without reporting later phases as executed.

## Stress-harness audit

**Verified defect in the old harness:** the daemon soak treated `ok:true` as a successful search regardless of whether its inserted symbol appeared. It edited and restored a source after 0.2 seconds while the watcher uses a longer debounce window. A 40-cycle baseline run did not observe its first drain until cycle 36; its plateau gate correctly failed with only three post-warm-up samples. This was not forty proved edit/update cycles.

The same missing visibility assertion affected standalone stress: edge-digest equality cannot detect a stale added function with no outgoing edges. Both modes now use one receipt validator, `rust/tools/soak-query-check.pl`, to require the exact symbol name and source path after insertion. After restoration they require its absence from a complete, internally consistent result. Capped, malformed or unavailable results cannot establish absence. Daemon mode holds each source state until both the expected symbol result and verified freshness arrive, with a validated convergence bound of 1–300 seconds (45 by default), plus the existing separately bounded IPC exchanges.

The regression executes the actual shell `ask` helper with controlled protocol responses, not a separate copy of its validation rule. The old helper accepted empty positives, another file's name, a prefix namesake, a still-present deleted name, capped negatives and malformed result payloads; eleven adversarial subcases failed before the fix. Nine tests now pass, including the shared standalone receipt contract and an actual one-second convergence timeout when the symbol never appears. The checker uses Perl/JSON::PP already required by this harness and adds no dependency.

**Verified baseline stress:** the immutable clean release passed the original standalone 40-cycle graph-digest/RSS/store plateau test. After hardening the assertions, a two-cycle actual daemon run passed exact insertion/removal checks, final freshness and graph-digest restoration. That short smoke explicitly did not assert growth. Initial fixture-root and missing-IPC-probe setup failures, the insufficient-warm-up failure, and successful reruns are all preserved in `.devcouncil/audit/update-soak/`; none of the refused checks is labeled passed.

The CLI progress integration suite passed all 26 tests in release mode. `python3 tests/unit/test_soak_query_check.py` passed all nine harness tests; `bash -n rust/tools/soak.sh`, `perl -c rust/tools/soak-query-check.pl`, formatting and diff-whitespace checks passed. The baseline benchmark's global unresolved/SQL/PowerShell disclosures remain valid limitations; these selected tests do not establish complete language coverage, complete caller recall or universal robustness.

## Frozen candidate profile and completed stress

**Verified:** the final profile used clean release `eed3872f` with SHA-256 `afff0036ffa5e1d281e9d6852a743533b03dc4b303545532e1485f602f443e3e`. Its source/provenance is preserved in `.devcouncil/audit/candidate-final/`; the profile command and complete source manifest are in `.devcouncil/audit/update-profile/final-candidate/`. All 1,186 tracked source hashes matched after restoration, all seven exact inserted symbol/file checks succeeded, and the removed probe's complete query was empty. An earlier exploratory instrumented run is retained separately with its uncertain source-snapshot scope and is not the source of these final measurements.

The final seven-edit median was **802.8 ms** (695.3–815.1 ms), with a native reported total of **743.4 ms**. This does not demonstrate a latency improvement over the earlier clean baseline, and no speedup claim is made. The baseline and candidate runs occurred at different times; the candidate includes the audit's attribution/retention fixes. They were not an interleaved experiment that isolates those individual changes.

| Final phase or nested operation | Median ms |
|---|---:|
| Entire scan/extract/setup stage | 199.9 |
| Cached extraction / parsing | 110.2 |
| Resolver index construction | 39.8 |
| Discovery and file reading | 23.7 |
| Go-module discovery | 12.3 |
| Affected write-set computation | 7.4 |
| Pending reconciliation | 1.3 |
| Store opening | 0.8 |
| Prior file hashes | 0.3 |
| Writer-lock waiting | 0.1 |
| Global resolution | 217.0 |
| Global analysis | 66.9 |
| Persistence/reclaim | 259.5 |
| Unresolved-row writing, within persistence | 122.1 |
| Generation pruning, within persistence | 42.6 |
| Vacuum/checkpoint, within persistence | 8.9 |

A separately observed interval from receipt of the complete JSON output line to process exit had a **51.7 ms** median (45.0–52.8 ms). Thus much of the time outside the native reported total occurs after its result is written. **Inferred:** destruction/teardown is a plausible contributor; this experiment does not split individual destructors or establish a safe teardown optimization. The new per-operation timings exclude writer contention and affected-set construction as dominant causes in this fixture. Cached extraction, global resolution and persistence remain the substantial work.

The frozen `d522fadc9147` candidate passed **40 standalone cycles and 40 daemon cycles** with exact insertion/removal symbol/file checks, graph-digest restoration and plateau assertions. Standalone half-means were RSS 28.530→28.459 MB and store 1.637→1.647 MB. Daemon half-means were RSS 31.355→31.703 MB and database-plus-WAL 19.310→16.660 MB; daemon peak RSS was 31.982 MB. Both used the existing 10% plateau ceiling. The daemon's initial WAL growth leveled out, and its five-minute maintenance later reclaimed space; the observed live allocation was not unbounded growth.

The digest compares source symbol, target symbol and edge kind. It does not establish equality of confidence, resolution provenance, node properties, unresolved ledgers or coverage metadata. Exact edit visibility and freshness are separate checks; the plateau is scoped to this fixture and these 40 cycles.

A subsequent adversarial review found that an unavailable search receipt with an empty result could still satisfy the shared checker. Its new regression failed before correction. The checker now requires `resolution: Available` for both presence and absence, independently of the status/ordinary-envelope checks. **All ten final harness tests pass**, and the final `eed3872f` binary passed a further two-cycle actual daemon smoke using that availability gate. The 40-cycle runs' earlier harness revision is preserved with SHA-256 hashes in `candidate40-harness.json`; their validator was not modified during those runs. The two-cycle follow-up does not claim a new plateau measurement.

**Verified remaining coverage:** the final restored status is fresh at store schema 20, with 10,193 nodes and 33,789 edges. It still lists **eight SQL import-extractor gaps and one PowerShell pattern fallback**. Its inspected explore response explicitly warns that **92,904 of 162,590 unresolved attribution sites** remain after excluding 69,686 classified sites. That remaining count increased from the older 91,784 because more conservative attribution preserves uncertainty; it is not evidence of complete resolution. The warning is repository-wide and not a measured number of missing callers for the selected target.

### Read-only trace of the interval after JSON output

**Verified source ordering:** the successful build explicitly drops its writer lock at `rust/devmap-cli/src/main.rs:4820`, calls `progress.complete` at line 4821, then emits JSON at line 4823. Completion invokes the renderer's finish operation (`main.rs:452`; `progress.rs:223`). `Display::finish` returns immediately if its finished flag is already set (`progress.rs:224`), so the later `Display::drop` at line 282 cannot repeat its bounded renderer wait. Neither the writer-lock release nor that finish wait explains time observed after the JSON line.

The build arm ends at `main.rs:4914`, dropping the owned analysis, resolution, resolver indices, extractions and store. `Resolver` owns many maps (`rust/devmap-resolve/src/resolver.rs:38`); `ResolutionResult` owns edge/unresolved collections (`rust/devmap-resolve/src/model.rs:681`); `Store` owns its SQLite connection (`rust/devmap-store/src/db.rs:643`). Runtime shutdown follows the `#[tokio::main]` function. These are concrete remaining owners of work after result emission, but their individual costs were not measured.

The recorded definition-query processes use the same runtime entry point and have approximately 1.0 ms between JSON and exit; the cold build has 58.5 ms and the restored build 47.3 ms. **Inferred:** the retained build data and connection cleanup are stronger suspects than a fixed runtime delay. The next targeted measurement should bracket individual successful-path drops of those large owners while preserving existing error cleanup, writer-lock release ordering and cancellation behavior. No teardown optimization has been made or justified by this audit.

### Shutdown hardening and final harness verification

**Verified defect:** the daemon harness formerly called `wait "$TIME_PID"` without a deadline after TERM, ignored its exit status, and did not confirm termination in its EXIT cleanup. The new process regressions ran against the preserved pre-fix script: six of seven failed. A TERM-ignoring child exceeded the six-second test guard, cleanup left running children, an exited timer hid a reparented child, and a daemon exiting with status 7 was accepted. The graceful control passed.

The harness now creates an owned process session before executing its timer, using the already installed Perl POSIX module. It requires the returned session ID to equal the launcher PID. Startup captures the timer's direct child; cleanup targets only the owned launcher and process group, which also contains a child reparented after an early timer exit. Normal shutdown signals the daemon first to preserve the timer's RSS receipt. It waits for both launcher and group exit for a validated 1–30-second grace period (10 by default), escalates to KILL if needed, and permits two more seconds to confirm termination. Forced or unconfirmed termination, unidentified startup ownership and nonzero timer exit status all fail the run. A shell `wait` occurs only after observing that the timer exited, and repeated cleanup preserves its original failure status.

The daemon handles TERM as a successful return (`devmap-serve/src/daemon.rs:1465`; `devmap-cli/src/main.rs:6187`), so an unexpected status is not accepted as graceful termination. The local Perl contract was checked in `POSIX.pm:401–402`, `POSIX.pod:1609–1612` and `perlipc.pod:542–547`; the last explicitly documents the `-1` failure and session-creation precondition. No dependency was added.

**Verified:** all eight real-process shutdown tests pass, including a deliberately refused KILL, an orphaned child, idempotent cleanup and an unrelated process that must survive. The eighth test delays `setsid` and invokes cleanup before the session exists. It first failed because the launcher remained alive while group-only observation reported exit. Cleanup now observes the launcher separately and kills it before the group, preventing it from creating descendants after group termination. The entire eight-test suite then passed in 7.468 seconds.

All ten exact-result/availability/freshness tests also passed. Shell and Perl syntax checks and `git diff --check` passed on the preceding seven-test harness revision, which also passed a two-cycle actual daemon smoke. After the final startup-race correction, the current eight-test helper passed another two-cycle actual daemon smoke using frozen `eed3872f`: exact insertion/removal checks, freshness, digest restoration, graceful status 0 and a retained 29,802,496-byte peak-RSS receipt. The helper, receipt checker, RSS helper and process-test source hashes matched before and after that run. Their snapshots, binary/probe identities and hashed stdout/stderr/CSV are recorded in `shutdown-eight-harness.json`. These Unix process tests were executed on macOS; Linux execution remains unverified. The new smoke does not repeat the 40-cycle plateau measurement, and the harness is not claimed to have a global timeout around every standalone command.

The pre-fix source, failing and passing logs, final harness hashes and smoke CSV are preserved under `.devcouncil/audit/update-soak/` with the `shutdown-` prefix. The previous 40-cycle receipts remain tied to their preserved earlier harness; they are not relabeled as runs of this final shutdown implementation.

## Final v51 source qualification

The later clean release at `962ea5f07921ea925e3b70821c5487472c8b57ea` was
profiled after all compilers finished, using the same seven-edit frozen-candidate
script and restored 1,186-file corpus. Process median was 804.0 ms (771.0–865.6),
native total 745.6 ms; scan/extract 206.3 ms, resolution 214.6 ms, analysis 73.0 ms,
persistence 244.2 ms. Nested extraction 113.6 ms, resolver index 40.1 ms, unresolved
write 119.4 ms, prune 40.1 ms and vacuum 8.5 ms remain material; writer wait 0.149 ms
remains small. The post-JSON median was 51.4 ms, without per-owner destructor
attribution. Nested times and separate medians must not be added.

Raw outputs, all seven samples, source hashes, binary identity and summary are
in `.devcouncil/audit/update-profile/delivery-v51/`. This later run was not
interleaved with CodeGraph or either earlier DevMap run; no speedup or regression
is inferred from comparing them. The final DevMap-only three-repository campaign
and its exact release provenance are described in `BENCHMARK_HARDENING_AUDIT.md`.

The same final release also passed a two-cycle real-daemon smoke with the final
strict shutdown helper: exact insertion/removal, availability/freshness, stable
restored digest, graceful zero exit and unchanged source/input hashes. The
30,621,696-byte RSS receipt, IPC probe identity and all outputs are retained in
`.devcouncil/audit/delivery-daemon-smoke/`. This does not repeat the 40-cycle
plateau run.
