#!/usr/bin/env bash
# Large-corpus memory-model probe, updated for dead-hardening AmbiguousGlobal
# semantics (2026-09-11):
#
#   * at or below AMBIGUOUS_FANOUT_CAP: emit one edge per candidate (SC4)
#   * above the cap: emit **no** edges; one unresolved ledger row carries the
#     full candidate list
#
# The pre-hardening probe assumed above-cap sites still emitted CAP edges while
# holding the full candidate Arc — that model is what this rewrite retires.
#
# WHAT IT MEASURES NOW
# --------------------
# 1. At-cap ambiguous vs unique control: bytes per candidate while edges are
#    still emitted (the Arc-sharing guard for the in-ceiling regime).
# 2. Above-cap ambiguous corpus: fan-out edges must be zero, and peak RSS must
#    not grow like the old capped-edge fan-out (which is how SC3-scale memory
#    returned).
#
#   usage: tools/memory_model_probe.sh
#   env:   DEVMAP_PROBE_DEFS DEVMAP_PROBE_CALLERS DEVMAP_PROBE_FNS DEVMAP_PROBE_CALLEES
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=tools/peak_rss.sh
. tools/peak_rss.sh

TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
DEVMAP="${DEVMAP_BIN:-$TARGET_DIR/release/devmap}"
[ -x "$DEVMAP" ] || { echo "PROBE FAIL: no devmap binary at $DEVMAP"; exit 1; }
command -v sqlite3 >/dev/null 2>&1 || { echo "PROBE FAIL: sqlite3 is not on PATH"; exit 1; }

CALLERS=${DEVMAP_PROBE_CALLERS:-100}
FNS=${DEVMAP_PROBE_FNS:-10}
CALLEES=${DEVMAP_PROBE_CALLEES:-5}

FANOUT_CAP=$(grep -oE 'AMBIGUOUS_FANOUT_CAP: usize = [0-9]+' \
  devmap-resolve/src/model.rs | grep -oE '[0-9]+$')
[[ "$FANOUT_CAP" =~ ^[1-9][0-9]*$ ]] || {
  echo "PROBE FAIL: cannot read AMBIGUOUS_FANOUT_CAP from devmap-resolve/src/model.rs"
  exit 1; }

# At-cap shape: edges == candidates. Override DEFS only when probing a custom
# in-ceiling width; default pins to the cap so the probe tracks the constant.
DEFS=${DEVMAP_PROBE_DEFS:-$FANOUT_CAP}
[ "$DEFS" -le "$FANOUT_CAP" ] || {
  echo "PROBE FAIL: DEVMAP_PROBE_DEFS=$DEFS is above AMBIGUOUS_FANOUT_CAP=$FANOUT_CAP; the at-cap leg must stay in-ceiling (above-cap is a separate leg below)"
  exit 1; }

# Above-cap width for the ledger-only leg (4x the ceiling — was the default
# DEFS=100 under the old probe).
ABOVE_DEFS=$((FANOUT_CAP * 4))

# At-cap residual cap (milli-bytes per candidate). Measured 2026-09-11 on the
# at-cap shape (edges == candidates): ~774 B/candidate for 80k sites×16. The
# retired two-term model predicted (694+86)=780 B on that collinear shape, so
# 1200 B (~1.5x) is the Arc-sharing guard — a clone-the-list regression still
# costs orders of magnitude more.
CANDIDATE_CAP_MILLI=1200000
# Above-cap peak may not exceed at-cap peak by more than this ratio. Holding
# candidate lists only long enough to write a ledger row must not recreate the
# old edge-fan-out memory curve.
ABOVE_RATIO_MAX_PCT=200
TIME_BUDGET_S=480

for pair in "DEFS=$DEFS" "CALLERS=$CALLERS" "FNS=$FNS" "CALLEES=$CALLEES" "ABOVE_DEFS=$ABOVE_DEFS"; do
  [[ "${pair#*=}" =~ ^[1-9][0-9]*$ ]] || {
    echo "PROBE FAIL: $pair is not a positive integer"
    exit 1; }
done

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

generate() { # <dir> <mode> <defs>
  local dir=$1 mode=$2 defs=$3 k=0 c=0 j p
  mkdir -p "$dir/defs" "$dir/callers"
  while [ "$k" -lt "$defs" ]; do
    {
      j=0
      while [ "$j" -lt "$CALLEES" ]; do
        if [ "$mode" = ambiguous ]; then
          printf 'pub fn shared_%s() -> u32 { %s }\n' "$j" "$j"
        else
          printf 'pub fn shared_%s_%s() -> u32 { %s }\n' "$k" "$j" "$j"
        fi
        j=$((j + 1))
      done
    } > "$dir/defs/def_$k.rs"
    k=$((k + 1))
  done
  while [ "$c" -lt "$CALLERS" ]; do
    {
      p=0
      while [ "$p" -lt "$FNS" ]; do
        printf 'pub fn caller_%s_%s() -> u32 {\n' "$c" "$p"
        j=0
        while [ "$j" -lt "$CALLEES" ]; do
          if [ "$mode" = ambiguous ]; then
            printf '    let _ = shared_%s();\n' "$j"
          else
            printf '    let _ = shared_0_%s();\n' "$j"
          fi
          j=$((j + 1))
        done
        printf '    0\n}\n'
        p=$((p + 1))
      done
    } > "$dir/callers/caller_$c.rs"
    c=$((c + 1))
  done
}

field() { # <metrics line> <key>
  printf '%s\n' "$1" | tr ' ' '\n' | awk -F= -v k="$2" '$1==k {print $2; exit}'
}

SITES=$((CALLERS * FNS * CALLEES))
FILES_AT=$((DEFS + CALLERS))
FILES_ABOVE=$((ABOVE_DEFS + CALLERS))
EXPECT_EDGES_AT=$((SITES * DEFS))
EXPECT_CAND_AT=$((SITES * DEFS))

echo "CAPPED: at-cap leg — ${FILES_AT} files, ${SITES} sites, ${DEFS} candidates (= AMBIGUOUS_FANOUT_CAP=${FANOUT_CAP})"
echo "CAPPED: above-cap leg — ${FILES_ABOVE} files, ${SITES} sites, ${ABOVE_DEFS} candidates (must emit 0 fan-out edges)"
echo "CAPPED: production corpora are NOT built here; raise DEVMAP_PROBE_CALLERS to scale locally."

START=$(date +%s)

measure_pair() { # <defs> <tag>
  local defs=$1 tag=$2 rss_amb rss_ctl m_amb m_ctl
  generate "$TMP/$tag.amb" ambiguous "$defs"
  generate "$TMP/$tag.ctl" unique "$defs"
  rss_amb=$(peak_rss_bytes "$TMP/$tag.amb.time" "$DEVMAP" --db "$TMP/$tag.amb.sqlite" --progress never build "$TMP/$tag.amb") || {
    echo "PROBE FAIL: could not measure peak RSS for the $tag ambiguous build" >&2; return 1; }
  rss_ctl=$(peak_rss_bytes "$TMP/$tag.ctl.time" "$DEVMAP" --db "$TMP/$tag.ctl.sqlite" --progress never build "$TMP/$tag.ctl") || {
    echo "PROBE FAIL: could not measure peak RSS for the $tag control build" >&2; return 1; }
  m_amb=$(tools/fanout.sh "$TMP/$tag.amb.sqlite") || {
    echo "PROBE FAIL: fan-out metrics unavailable for the $tag ambiguous build" >&2; return 1; }
  m_ctl=$(tools/fanout.sh "$TMP/$tag.ctl.sqlite") || {
    echo "PROBE FAIL: fan-out metrics unavailable for the $tag control build" >&2; return 1; }
  printf '%s %s %s %s %s %s %s %s %s\n' \
    "$rss_amb" \
    "$rss_ctl" \
    "$((rss_amb - rss_ctl))" \
    "$(field "$m_amb" fanout_edges)" \
    "$(field "$m_amb" candidates)" \
    "$(field "$m_amb" fanout_sites)" \
    "$(field "$m_amb" fanout_max_n)" \
    "$(field "$m_amb" max_candidates)" \
    "$(field "$m_ctl" fanout_sum_n2)"
}

read -r AT_RSS_AMB AT_RSS_CTL AT_DELTA AT_EDGES AT_CAND AT_SITES AT_MAX AT_MAX_CAND AT_CTL_N2 \
  < <(measure_pair "$DEFS" atcap) || exit 1

read -r AB_RSS_AMB AB_RSS_CTL AB_DELTA AB_EDGES AB_CAND AB_SITES AB_MAX AB_MAX_CAND AB_CTL_N2 \
  < <(measure_pair "$ABOVE_DEFS" abovcap) || exit 1

ELAPSED=$(( $(date +%s) - START ))

echo "probe: at-cap — sites=${AT_SITES} edges=${AT_EDGES} candidates=${AT_CAND} max_n=${AT_MAX} max_cand=${AT_MAX_CAND} delta=$((AT_DELTA/1024/1024)) MiB"
echo "probe: above-cap — sites=${AB_SITES} edges=${AB_EDGES} candidates=${AB_CAND} max_n=${AB_MAX} max_cand=${AB_MAX_CAND} delta=$((AB_DELTA/1024/1024)) MiB"
echo "probe: elapsed ${ELAPSED}s"

FAIL=0

# --- at-cap arithmetic -------------------------------------------------------
[ "$AT_SITES" -eq "$SITES" ] || {
  echo "PROBE FAIL: at-cap derived $AT_SITES sites, corpus has $SITES"; FAIL=1; }
[ "$AT_MAX" -eq "$DEFS" ] || {
  echo "PROBE FAIL: at-cap widest fan-out $AT_MAX, expected $DEFS"; FAIL=1; }
[ "$AT_MAX_CAND" -eq "$DEFS" ] || {
  echo "PROBE FAIL: at-cap widest candidate list $AT_MAX_CAND, expected $DEFS"; FAIL=1; }
[ "$AT_EDGES" -eq "$EXPECT_EDGES_AT" ] || {
  echo "PROBE FAIL: at-cap edges $AT_EDGES, expected $EXPECT_EDGES_AT"; FAIL=1; }
[ "$AT_CAND" -eq "$EXPECT_CAND_AT" ] || {
  echo "PROBE FAIL: at-cap candidates $AT_CAND, expected $EXPECT_CAND_AT"; FAIL=1; }
[ "$AT_CTL_N2" -eq 0 ] || {
  echo "PROBE FAIL: at-cap control has fan-out Sum(N^2)=$AT_CTL_N2"; FAIL=1; }
[ "$AT_DELTA" -gt 0 ] || {
  echo "PROBE FAIL: at-cap ambiguous build did not cost more than its control ($AT_RSS_AMB vs $AT_RSS_CTL)"; FAIL=1; }

AT_MILLI=$((AT_DELTA * 1000 / AT_CAND))
echo "probe: ${AT_MILLI} milli-bytes/candidate at-cap (cap ${CANDIDATE_CAP_MILLI})"
[ "$AT_MILLI" -lt "$CANDIDATE_CAP_MILLI" ] || {
  echo "PROBE FAIL: ${AT_MILLI} milli-bytes/candidate >= cap ${CANDIDATE_CAP_MILLI}"; FAIL=1; }

# --- above-cap: zero edges, ledger only --------------------------------------
[ "$AB_EDGES" -eq 0 ] || {
  echo "PROBE FAIL: above-cap emitted $AB_EDGES fan-out edges; AmbiguousGlobal above the ceiling must emit none"; FAIL=1; }
[ "$AB_SITES" -eq 0 ] || {
  echo "PROBE FAIL: above-cap fanout_sites=$AB_SITES; edge-derived sites must be zero when no edges emit"; FAIL=1; }
[ "$AB_CTL_N2" -eq 0 ] || {
  echo "PROBE FAIL: above-cap control has fan-out Sum(N^2)=$AB_CTL_N2"; FAIL=1; }

# Ledger must record every above-cap call site (resolution AmbiguousGlobal).
LEDGER=$(sqlite3 -readonly "$TMP/abovcap.amb.sqlite" \
  "SELECT COUNT(*) FROM unresolved_rows WHERE reason LIKE 'AmbiguousGlobal%' AND valid_to IS NULL;") || {
  echo "PROBE FAIL: could not count AmbiguousGlobal ledger rows"; exit 1; }
[ "$LEDGER" -eq "$SITES" ] || {
  echo "PROBE FAIL: above-cap ledger has $LEDGER AmbiguousGlobal rows, corpus has $SITES call sites"; FAIL=1; }

# Memory must not explode when candidates stop becoming edges.
if [ "$AT_DELTA" -gt 0 ]; then
  ABOVE_RATIO_PCT=$((AB_DELTA * 100 / AT_DELTA))
else
  ABOVE_RATIO_PCT=0
fi
echo "probe: above-cap delta is ${ABOVE_RATIO_PCT}% of at-cap delta (max ${ABOVE_RATIO_MAX_PCT}%)"
[ "$ABOVE_RATIO_PCT" -le "$ABOVE_RATIO_MAX_PCT" ] || {
  echo "PROBE FAIL: above-cap memory grew to ${ABOVE_RATIO_PCT}% of at-cap (max ${ABOVE_RATIO_MAX_PCT}%) — candidates are still being materialised like edges"; FAIL=1; }

[ "$ELAPSED" -le "$TIME_BUDGET_S" ] || {
  echo "PROBE FAIL: probe took ${ELAPSED}s, budget ${TIME_BUDGET_S}s"; FAIL=1; }

[ "$FAIL" -eq 0 ] || exit 1
echo "MEMORY MODEL OK (${AT_MILLI} milli-B/candidate at-cap, above-cap ledger=${LEDGER}, ratio ${ABOVE_RATIO_PCT}%)"
