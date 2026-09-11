#!/usr/bin/env bash
# Ambiguity fan-out metrics for a devmap store, derived from the persisted
# graph rather than from the resolver's own source.
#
#   usage: tools/fanout.sh <store.sqlite>
#   stdout (one line): files=<n> fanout_edges=<n> fanout_sum_n2=<n> \
#                      fanout_sites=<n> fanout_max_n=<n> \
#                      candidates=<n> candidate_n2=<n> max_candidates=<n>
#
# EDGES ARE NOT CANDIDATES. `AMBIGUOUS_FANOUT_CAP` bounds how many edges one
# site emits; it does not bound the candidate list the `Arc<Resolution>` holds,
# which is what resolver memory is actually proportional to. The `fanout_*`
# fields count emitted edges and the `candidate*` fields count what was weighed.
# Since audit R-7 those are different numbers, and a coefficient computed
# against the first has the wrong denominator — see `tools/fanout.sql`.
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
#   * `pre_v16` > 0 — an ambiguous edge written before `candidate_total`
#     existed. NULL there means "not recorded", and a sum that treated it as
#     zero would understate the denominator and inflate every coefficient
#     computed from it. Refusing is the fail-closed direction: rebuild the store
#     with a current binary.
set -euo pipefail

DB=${1:?usage: fanout.sh <store.sqlite>}
HERE=$(cd "$(dirname "$0")" && pwd)
SQL="$HERE/fanout.sql"

[ -r "$DB" ] || { echo "fanout: store is not readable: $DB" >&2; exit 1; }
[ -r "$SQL" ] || { echo "fanout: missing $SQL" >&2; exit 1; }
command -v sqlite3 >/dev/null 2>&1 || { echo "fanout: sqlite3 is not on PATH" >&2; exit 1; }

ROW=$(sqlite3 -readonly "$DB" < "$SQL") || {
  echo "fanout: query failed against $DB" >&2; exit 1; }

IFS='|' read -r FILES EDGES SUM_N2 SITES MAX_N UNJOINED ALT_SUM_N2 ALT_SITES \
  CANDIDATES CANDIDATE_N2 MAX_CANDIDATES PRE_V16 <<EOF
$ROW
EOF

for v in "$FILES" "$EDGES" "$SUM_N2" "$SITES" "$MAX_N" "$UNJOINED" "$ALT_SUM_N2" "$ALT_SITES" \
         "$CANDIDATES" "$CANDIDATE_N2" "$MAX_CANDIDATES" "$PRE_V16"; do
  [[ "$v" =~ ^[0-9]+$ ]] || {
    echo "fanout: malformed result row from $DB: '$ROW'" >&2; exit 1; }
done

[ "$UNJOINED" -eq 0 ] || {
  echo "fanout: $UNJOINED fan-out edge(s) join no node — the callee name behind them is unknown" >&2
  exit 1; }
[ "$SUM_N2" -eq "$ALT_SUM_N2" ] && [ "$SITES" -eq "$ALT_SITES" ] || {
  echo "fanout: the two independent derivations disagree (sum_n2 $SUM_N2 vs $ALT_SUM_N2, sites $SITES vs $ALT_SITES) — the grouping key no longer describes the store" >&2
  exit 1; }
[ "$PRE_V16" -eq 0 ] || {
  echo "fanout: $PRE_V16 ambiguous edge(s) carry no candidate_total — written before schema v16; the candidate denominator is not recoverable from this store" >&2
  exit 1; }
[ "$FILES" -gt 0 ] || { echo "fanout: the latest generation indexed no files" >&2; exit 1; }
# Candidates cannot be fewer than the edges they produced: the resolver emits
# min(candidates, AMBIGUOUS_FANOUT_CAP) edges per site, so `>=` holds by
# construction and a violation means the column is not carrying what this script
# thinks it is.
[ "$CANDIDATES" -ge "$EDGES" ] || {
  echo "fanout: $CANDIDATES candidates against $EDGES emitted edges — a site cannot emit more edges than it weighed candidates" >&2
  exit 1; }

printf 'files=%s fanout_edges=%s fanout_sum_n2=%s fanout_sites=%s fanout_max_n=%s candidates=%s candidate_n2=%s max_candidates=%s\n' \
  "$FILES" "$EDGES" "$SUM_N2" "$SITES" "$MAX_N" "$CANDIDATES" "$CANDIDATE_N2" "$MAX_CANDIDATES"
