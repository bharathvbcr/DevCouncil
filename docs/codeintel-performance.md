# Code-intelligence performance ratchets

The code-intelligence benchmark uses generated Python repositories with stable
paths, source text, import topology, and changed-file selection. It exercises a
real schema-v2 cold build, a localized one-file sync, SQLite persistence, the
compatibility JSON export, and search/explore/dead queries.

## Profiles

- `fast` generates 256 files across eight packages. It is the pull-request
  ratchet and normally completes in a few seconds.
- `heavy` generates exactly 10,000 files across 40 packages. It is an explicit
  release or scheduled profile and is not part of the default test suite.

Run the profiles with:

```shell
uv run pytest tests/performance -q
uv run python scripts/codeintel-benchmark.py --profile heavy \
  --output artifacts/codeintel-heavy.json
```

The script writes both JSON and a Markdown summary. It exits nonzero when any
ratchet fails and names the metric, observed value, and configured ceiling.
Pass `--fixture-root PATH` to retain the generated repository for inspection.

The code-intelligence platform workflow runs the fast profile on Linux, macOS
Intel/ARM, and Windows for every push and pull request. The 10,000-file profile
runs nightly and can also be requested explicitly with the
`run_heavy_profile` workflow input; its JSON and Markdown reports are uploaded
as CI artifacts.

## Measurements

- Cold and one-file wall times use `time.perf_counter()`.
- Peak RSS uses the process high-water mark and is most meaningful from the
  isolated benchmark script.
- Database bytes use SQLite page size multiplied by page count after a WAL
  checkpoint. Freelist bytes and ratio use `PRAGMA freelist_count`.
- The database-to-export ratio compares allocated SQLite pages with the
  compatibility graph JSON. SQLite intentionally also contains compressed
  source, analysis shards, FTS, retained generation memberships, and indexes.
- Row-write counts are the newly inserted content-addressed node, edge, and
  dead-code payload rows reported by the schema-v2 store. Membership totals are
  retained in the artifact for diagnosis.
- Query p50/p95 measurements cover SQLite FTS search and uncached
  explore/dead graph queries.

Thresholds live in `tests/performance/thresholds.json`. Exact fixture size,
package count, schema version, affected-file count, payload-write count, size
ratio, and freelist ratio are deterministic ratchets. Wall time, RSS, and query
latency use conservative machine-sensitive ceilings and should be recalibrated
from isolated local runs before being tightened.

## July 2026 local calibration

On an Apple Silicon development machine, the fast profile cold-indexed in
0.70s, synced one file in 0.16s, used 75.1 MiB peak RSS, and recorded query p95
latencies of 0.87ms search, 32.21ms explore, and 39.72ms dead. SQLite occupied
2,809,856 bytes versus 534,898 compatibility-export bytes (5.253x), with a
1.603% freelist ratio.

The explicit 10,000-file profile cold-indexed in 84.33s, synced one file in
14.33s, used 640.3 MiB peak RSS, and recorded query p95 latencies of 0.84ms
search, 1,241.76ms explore, and 1,095.57ms dead. SQLite occupied 102,535,168
bytes versus 20,984,044 export bytes (4.886x), with a 1.754% freelist ratio.

The localized payload-write defect from that calibration is resolved. A
2026-07-15 rerun wrote zero payload rows for both profiles, affected exactly one
file, and passed every configured ratchet. The fast rerun cold-indexed in
0.718s, synced in 0.120s, used 76.2 MiB peak RSS, and recorded query p95 values
of 0.57ms search, 29.66ms explore, and 36.89ms dead. The heavy rerun synced in
12.805s with zero new payloads and no violations.
