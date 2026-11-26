# Workbench retention and compaction

`store.compact` (workbench method, Go `WorkbenchMethods`, CLI `dcstore work --method store.compact`)
is the only thing that deletes from `work_requests`, `work_events` and `work_revisions`.
It needs no schema change: every rule is a predicate over existing rows. Nothing runs
automatically; a host calls it (for example at startup, or daily).

## Policy

| Table | Rows deleted | Never deleted |
|---|---|---|
| `work_requests` (idempotency receipts) | older than `receipt_window_seconds` (default 7 days, minimum 1 hour), age taken from the `at` of the receipt's own event | receipts of `runs.claim`, `notifications.claim`, `decisions.claim` (their replay answer is `claim_consumed`, which needs the receipt); a receipt whose event is missing (age unknown, fail closed) |
| `work_events` | older than `event_retention_seconds` (default 30 days, never shorter than the receipt window) and not among the newest `keep_events` (default 1000, minimum 1) | any event a surviving receipt points at; any event that is a `work_attention.source_sequence` (foreign key); the newest event (it is the cursor `max(sequence)`; sequences are AUTOINCREMENT and never reused) |
| `work_revisions` | revisions more than `keep_revisions` (default 50, minimum 1) behind the newest of the same record | the newest `keep_revisions` of every record (the newest is the live row an `expected_revision` is checked against); every revision of a decision in `pending`, `decided` or `dispatching`; every revision of a run in `prepared`, `starting`, `running` or `unresolved` |

Live tables (`work_items`, `work_runs`, `work_decisions`, ...) are never touched, so an
unresolved decision and its run survive any compaction.

Consequences hosts must accept:

- A retry whose `request_id` is older than the receipt window no longer replays its
  receipt. It is judged as a new request, and `expected_revision` (checked against the
  live row, never against `work_revisions`) rejects it as `revision_conflict` when the
  record has moved. Retry within the window, which is why it cannot be set under an hour.
- `events.list` cursors older than the retained log skip the pruned gap; the sequence
  numbers stay monotonic. `items.history` returns only retained revisions.

## Bounds

One call deletes at most `limit` rows per table (default 10,000, maximum 100,000). The
reply reports, per table, `deleted`, `eligible` and `truncated`, so a capped pass is
never mistaken for a complete one; call again until `truncated` is false. Receipts are
pruned before events in the same transaction, which is what lets an event become
eligible. All parameters are validated; unknown fields are refused.

## Schema-upgrade backups

An upgrade copies the profile to `<profile>.pre-schema-<version>.bak` (`VACUUM INTO`) before
its first step. Until this policy nothing ever removed one: a profile that crossed four
schema versions held four extra copies of itself.

- **Retention.** The newest **2** `pre-schema` backups are kept by default. Set
  `DC_STORE_KEEP_BACKUPS` to a positive number, or to `all` to keep every backup, or call
  `Store::set_backup_retention(BackupRetention::keep(n) | KeepAll)` on a handle (the handle's
  setting wins over the variable). `0` is refused: it would delete the backup an upgrade has
  just made. A value that cannot be read keeps every backup and says so on stderr: an
  unparseable setting must not become permission to delete.
- **When it runs.** Only right after an upgrade wrote a *new* backup and read it back
  (`PRAGMA quick_check`, and the schema version in `work_meta` must be the one in the name).
  A copy that fails that check is removed, never renamed to `.bak`, and the upgrade is refused.
  A plain open, a read, a current profile, and the resumption of an interrupted upgrade (whose
  backup already exists) delete nothing.
- **What is kept.** The backup just written always counts and is never removed. Older ones are
  walked newest schema first (numerically: 9 is older than 10); each must verify to count. One
  that does not verify is not counted, **not removed**, and reported. Everything beyond the
  count is removed. Only regular files named exactly `<profile>.pre-schema-<digits>.bak` are
  candidates: `pre-tasks-retire`, `*.partial`, directories and symlinks are never touched.
- **What is reported.** Each removal is written to stderr by path, and `store.upgrade` returns
  `pruned_backups: {removed, unverified, errors}` (`UpgradeReport.prune` in Rust).
- **Peak disk.** The new backup is written before the old ones go, so the peak is the
  retention count plus one profile-sized copies. Pruning frees nothing until the next upgrade.

## Returning free pages: `store.vacuum`

`store.compact` deletes rows; the pages come back to the operating system only under
`auto_vacuum=INCREMENTAL`, which a profile has only if it was created after that setting was
added. An older profile (mode NONE) keeps every freed page, and SQLite cannot change the mode
except by rewriting the file. `store.vacuum` is that rewrite, run deliberately:

```
dcstore --db <profile> work --method store.vacuum --input '{}'
```

Optional input: `reserve_bytes` (default 1 GiB), headroom required on top of the rewrite.
It is available through the Rust dispatcher and the dcstore CLI, and is deliberately not in
the Go `WorkbenchMethods` list: a full rewrite holds the write lock for as long as it takes.

- It sets `auto_vacuum=INCREMENTAL` before the one `VACUUM`, so afterwards `store.compact`
  returns pages itself and no second full rewrite is ever needed.
- It never runs on open, never inside a migration (an older profile is refused with
  `schema_upgrade_required`), and not inside a transaction (`invalid_input`).
- It refuses before touching a page when the disk cannot hold it: `insufficient_disk_space`
  (needs the database size again on the profile's filesystem *and* in SQLite's temporary
  directory, summed when they are one mount, plus `reserve_bytes`), or `disk_space_unknown`
  when free space could not be measured (`df -Pk`; there is no portable call in `std`, and
  this crate forbids `unsafe` and takes no dependency for it). A check that could not run is
  a refusal, not a pass. `vacuum_blocked`: another connection holds the profile.
- It reports `bytes_before/after` (the database as SQLite counts it), `file_bytes_before/after`
  (the file), `free_pages_before/after`, `auto_vacuum_before/after`, `required_bytes`,
  `available_bytes` and `checkpoint` (`blocked` means a reader kept the WAL from folding in,
  so the file shrinks at the next checkpoint). A profile that is already incremental with
  nothing free answers `not_needed` and is not rewritten.
- Order: run `store.compact` until nothing reports `truncated` (it frees the rows), then
  `store.vacuum` (it returns the pages), with the profile's other writers stopped.

## Tests

`rust/dc-store/tests/workbench_backups.rs` and `workbench_vacuum.rs` cover the two sections
above; `rust/dc-store/tests/workbench_retention.rs` has one test per invariant (receipt in and
out of window, claim receipts, revision bound plus `expected_revision`, open decisions
and live runs, event protection, per-pass bound, parameter refusal).
`dc/store/workbench_test.go` covers the host client. `BenchmarkWorkbenchProfile` in
`dc/store/workbench_bench_test.go` seeds 10,000 tasks with full history, measures
`items.list`/search/history/edit, then runs one compaction pass and checks its counts.

## Measured (Apple M5 Pro, one run, `-benchtime=20x`, machine under other load)

Command, from `backend/go_orchestrator`:
`go test ./dc/store/ -run '^$' -bench '^BenchmarkWorkbenchProfile$' -benchtime=20x -count=1 -timeout=60m -v`

Fixture: 10,000 tasks x 10 revisions, 100 repositories, 100,110 events and receipts
(seed 7m31s through the real host client). Per call, mean / p95:
global list 3.32 / 3.79 ms; workspace 8.29 / 9.31 ms; repository 1.90 / 2.33 ms;
search 10.2 / 12.95 ms; history 0.59 / 0.79 ms; edit 2.16 / 2.83 ms.
One `store.compact {"keep_revisions":1,"limit":100000}` pass deleted 90,021 revisions
(eligible 90,021, not truncated) in 2.94 s; receipts and events were all fresh so none
were eligible. List and search latency on that 10,000-task fixture is independent of
history size, so retention there is about disk growth and history paging, not list
latency (inferred from the unchanged query plans, not separately measured on a
compacted fixture).

`BenchmarkWorkbenchProfileStress` was run on 2026-10-13 (Apple M5 Pro, `-benchtime=20x`,
`-count=1`). Same mutation count, spread over 100,000 tasks with one revision each:
100,110 events, seed 8m28.6s, `latest_sequence` 100,110 (`events.list` total is only a
10,000 floor once the log exceeds that). Twenty iterations, mean / p95 in milliseconds:
global 9.63 / 11.65, workspace 76.23 / 112.9, repository 7.17 / 9.944, search 83.88 / 120.5,
history 0.606 / 1.160, rare search 2.39 / 5.646, repository search 57.80 / 72.87,
workspace search 115.9 / 150.5, empty status 2.92 / 7.977, edit 5.10 / 11.78.
One `store.compact {"keep_revisions":1,"limit":100000}` deleted 21 revisions (eligible 21,
not truncated) in 517 ms; receipts and events were fresh. The printed edit sample is 20
iterations. The high-water mark after edits was 100,131 (100,110 seed events plus 21
committed edits); the benchmark's revision counter includes any extra invocation of that
sub-benchmark. Workspace list and search are
slower at 100,000 tasks than at 10,000 tasks with the same number of mutations. The same
figures are in Manvi's "docs/WORKBENCH.md" (in the Manvi repo).

## Scale behaviour (hx/w4-store-scale)

- `repositories.get` takes `identity_key` instead of `id` (exactly one) and answers from
  the UNIQUE index: a host that knows a repository only by identity asks once.
- `items.history` takes `newest_first` (with `before_revision` as its cursor; mixing it
  with `after_revision` is refused), so the latest revision is one `limit:1` request.
- A receipt keeps `{ok, sequence, replay:"event", automatic_enhancement_queued}` and a
  replay rebuilds the response from the event it names, so a record body is stored in the
  row, `work_revisions` and `work_events` but no longer a fourth time in `work_requests`.
  Receipts written by older builds keep their full response and replay unchanged. A
  receipt whose event is gone answers `receipt_unreadable` (retention deletes receipts
  first, so this means the profile was edited by hand). An older build reading a newer
  receipt would replay the short form: upgrade every host sharing a profile together.
- `events.list` and `items.history` no longer run an unbounded `count(*)` per page. A last
  page is exact without a count; otherwise the count stops at 10,000 and `total_exact`
  says whether `total` is a floor.
- A profile created by this build is `auto_vacuum=INCREMENTAL`; `store.compact` then runs
  `incremental_vacuum(limit)` and reports `vacuum.{incremental,free_pages_before,free_pages_after}`.
  SQLite cannot change the mode of a file that already has tables without a full `VACUUM`,
  so existing profiles report `incremental:false` and keep their size until that is run.
- Measured by `cargo test -p dc-store --release --test workbench_scale_bench -- --ignored --nocapture`
  (5,000 repositories, one task with 5,000 4 KB revisions, Apple silicon, min of 5):
  find by identity 17-35 ms / 23 requests before, 0.04-0.05 ms / 1 after; last revision
  53-116 ms / 25 requests before, 0.21 ms / 1 after (two runs, machine under other load);
  receipt 77 bytes instead of body plus envelope; reopening the 71 MiB profile and
  answering one find 0.78 ms, so the MCP binary's per-call reopen is not the bottleneck
  and was left alone.
