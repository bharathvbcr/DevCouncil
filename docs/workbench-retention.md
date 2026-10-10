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

## Tests

`rust/dc-store/tests/workbench_retention.rs` has one test per invariant (receipt in and
out of window, claim receipts, revision bound plus `expected_revision`, open decisions
and live runs, event protection, per-pass bound, parameter refusal).
`dc/store/workbench_test.go` covers the host client. `BenchmarkWorkbenchProfile` in
`dc/store/workbench_bench_test.go` seeds 10,000 tasks with full history, measures
`items.list`/search/history/edit, then runs one compaction pass and checks its counts.
