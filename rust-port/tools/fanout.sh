#!/usr/bin/env bash
# Ambiguity fan-out metrics for a devmap store, derived from the persisted
# graph rather than from the resolver's own source.
#
#   usage: tools/fanout.sh <store.sqlite>
#   stdout (one line): files=<n> fanout_edges=<n> fanout_sum_n2=<n> \
#                      fanout_sites=<n> fanout_max_n=<n>
#
# FAIL-CLOSED. `verify.sh` already models the rule this follows: an unavailable
# peak-RSS reading fails its step rather than skipping it, because a check that
# could not run must never report what a check that ran and passed reports. The
# same applies here — every path out of this script is either a validated number
# or a non-zero exit. In particular the two validations below are refusals, not
# warnings:
#
#   * `unjoined` > 0 — a fan-out edge points at a node that is not in the store,
#     so the callee name behind it is unknown and the grouping is incomplete.
#   * `alt_*` disagreeing with the primaries — `tools/fanout.sql` recovers the
#     callee name two independent ways (node identity, and the suffix of the
#     qualified name). They agree on every corpus measured so far. If they ever
#     stop agreeing, the grouping key no longer describes the store, and the
#     honest response is to stop rather than to publish whichever number came
#     first.
set -euo pipefail

DB=${1:?usage: fanout.sh <store.sqlite>}
HERE=$(cd "$(dirname "$0")" && pwd)
SQL="$HERE/fanout.sql"

[ -r "$DB" ] || { echo "fanout: store is not readable: $DB" >&2; exit 1; }
[ -r "$SQL" ] || { echo "fanout: missing $SQL" >&2; exit 1; }
command -v sqlite3 >/dev/null 2>&1 || { echo "fanout: sqlite3 is not on PATH" >&2; exit 1; }

ROW=$(sqlite3 -readonly "$DB" < "$SQL") || {
  echo "fanout: query failed against $DB" >&2; exit 1; }

IFS='|' read -r FILES EDGES SUM_N2 SITES MAX_N UNJOINED ALT_SUM_N2 ALT_SITES <<EOF
$ROW
EOF

for v in "$FILES" "$EDGES" "$SUM_N2" "$SITES" "$MAX_N" "$UNJOINED" "$ALT_SUM_N2" "$ALT_SITES"; do
  [[ "$v" =~ ^[0-9]+$ ]] || {
    echo "fanout: malformed result row from $DB: '$ROW'" >&2; exit 1; }
done

[ "$UNJOINED" -eq 0 ] || {
  echo "fanout: $UNJOINED fan-out edge(s) join no node — the callee name behind them is unknown" >&2
  exit 1; }
[ "$SUM_N2" -eq "$ALT_SUM_N2" ] && [ "$SITES" -eq "$ALT_SITES" ] || {
  echo "fanout: the two independent derivations disagree (sum_n2 $SUM_N2 vs $ALT_SUM_N2, sites $SITES vs $ALT_SITES) — the grouping key no longer describes the store" >&2
  exit 1; }
[ "$FILES" -gt 0 ] || { echo "fanout: the latest generation indexed no files" >&2; exit 1; }

printf 'files=%s fanout_edges=%s fanout_sum_n2=%s fanout_sites=%s fanout_max_n=%s\n' \
  "$FILES" "$EDGES" "$SUM_N2" "$SITES" "$MAX_N"
