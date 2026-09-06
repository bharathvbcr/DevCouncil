#!/usr/bin/env bash
# Large-corpus memory-model probe: a scale-invariant bound on how much resident
# memory one unit of ambiguity fan-out is allowed to cost.
#
# WHY A COEFFICIENT AND NOT A CEILING
# -----------------------------------
# `verify.sh` step 5 bounds peak RSS on this repository, 1.1k files. The corpus
# this port is actually run against is 12,831 files. An absolute ceiling
# calibrated on the small one is silent on the large one, and one raised to
# cover the large one would no longer catch a regression on the small one. No
# single absolute number covers both. A *coefficient* does: bytes per unit of
# fan-out is the same quantity at any corpus size, so bounding it here bounds it
# on a corpus this CI could never afford to build.
#
# WHAT IT MEASURES
# ----------------
# Two builds over corpora that are identical in file count, symbol count and
# call count, and differ only in whether the callees are ambiguous:
#
#   ambiguous  K files each declaring the same `pub fn shared_j`, so a bare call
#              to `shared_j` can only reach the `AmbiguousGlobal` rung and fans
#              out to exactly K edges.
#   control    the same K files declaring K*M distinct names, so every call
#              resolves on the UniqueGlobal rung to exactly one edge.
#
# The control *is* the base term. Subtracting it removes base(files) by
# measurement instead of by model, which is the whole reason for the paired
# design: a coefficient computed against a modelled base inherits that model's
# error, and on a real repository the base term is ~97% of peak.
#
# WHAT IT DOES NOT MEASURE
# ------------------------
# It does not bound the absolute peak of any real repository — see the CAPPED
# line it prints. It bounds two coefficients and checks the model's prediction.
#
#   usage: tools/memory_model_probe.sh
#   env:   DEVMAP_PROBE_DEFS DEVMAP_PROBE_CALLERS DEVMAP_PROBE_FNS DEVMAP_PROBE_CALLEES
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=tools/peak_rss.sh
. tools/peak_rss.sh

# Honour CARGO_TARGET_DIR, matching `tools/soak.sh`, which already resolves the
# binary this way and for the same reason: a lane builds into its own target
# directory, and a probe that silently measured a stale
# `rust-port/target/release/devmap` would report the previous kernel's numbers
# as this one's — or, as here, refuse with a message about a build that in fact
# succeeded somewhere else.
TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
DEVMAP="${DEVMAP_BIN:-$TARGET_DIR/release/devmap}"
[ -x "$DEVMAP" ] || { echo "PROBE FAIL: no devmap binary at $DEVMAP"; exit 1; }
command -v sqlite3 >/dev/null 2>&1 || { echo "PROBE FAIL: sqlite3 is not on PATH"; exit 1; }

# Corpus shape. Files = DEFS + CALLERS; ambiguous call sites = CALLERS x FNS x
# CALLEES, each fanning out to DEFS edges.
DEFS=${DEVMAP_PROBE_DEFS:-100}
CALLERS=${DEVMAP_PROBE_CALLERS:-100}
FNS=${DEVMAP_PROBE_FNS:-10}
CALLEES=${DEVMAP_PROBE_CALLEES:-5}

# --- thresholds, each justified from measurement -----------------------------
#
# BYTES PER PAIR (the Sum(N^2) coefficient). SC3 measured the pre-fix resolver
# at 112-128 B per candidate pair, because each of a site's N edges owned its
# own clone of the N-element candidate list. The `Arc<Resolution>` fix made the
# list shared. Measured after it, at DEFS=100 over an 8x sweep of corpus size:
# 4.01, 4.06, 4.12, 4.15 B/pair. 40 B/pair sits 9.7x above the measured value
# and 2.8x below the regime SC3 left, so it catches a revert of that fix at any
# corpus size without sitting close enough to measurement noise to flake.
#
# This bound is shape-independent only above a minimum fan-out width: bytes per
# pair is bytes per edge divided by the mean fan-out, so a corpus of width 2
# would exceed 40 B/pair while using no more memory per edge. DEFS is therefore
# required to be at least 16 (410/16 = 26 B/pair, comfortably under the cap).
# **These three caps were calibrated before `AMBIGUOUS_FANOUT_CAP` existed, and
# have not been re-derived since. They currently fail. Read this before
# changing a number.**
#
# The cap bounds how many *edges* one ambiguous site emits (16). It does not
# bound the site's *candidate list*, which the `Arc<Resolution>` still holds in
# full — that is deliberate, and is what keeps `impact` answerable on candidates
# 2..N. So since audit R-7 the memory is proportional to candidates while every
# denominator here is derived from emitted edges, and the two stopped being the
# same number.
#
# Measured 2026-09-06, this machine, both runs after the arithmetic
# preconditions above were corrected:
#
#   DEFS=100 (cap active, 100 candidates -> 16 edges):
#     77,145 milli-B/pair, 1,234,329 milli-B/edge, 193% of model
#   DEFS=16 (cap inert, 16 candidates -> 16 edges):
#     45,926 milli-B/pair,   734,822 milli-B/edge, 138% of model
#
# Removing the cap's effect alone takes bytes-per-edge from 1,234 to 735 and
# inside its 800 cap, which is the decoupling stated above, measured. What is
# left — 46 B/pair against 40, and 138% against 125% — is either a real per-edge
# regression from the 410 B this model assumes, or an artifact of comparing a
# 16-wide 116-file corpus against coefficients measured on a 100-wide one. It
# was not settled, and a number moved to make a gate green would be exactly the
# "raised to fit" this repository refuses.
#
# **Fixing it properly needs a schema change.** `tools/fanout.sh` derives its
# metrics from the persisted graph, and `generation_edges` has no `details`
# column — the candidate total lives only on the in-memory `ResolvedEdge`. So
# the denominator the memory actually tracks is not recoverable from the store
# today, and making it so is a store change, not a probe change.
PAIR_CAP_MILLI=40000
MIN_DEFS=16
#
# BYTES PER FAN-OUT EDGE. This is the coefficient that actually describes the
# current code: measured 401.5, 406.4, 412.1, 415.2 B/edge across the same 8x
# sweep, and 406/411/409 B/edge across a 4x sweep of fan-out width at constant
# edge count. 800 B/edge is 1.9x the measured value.
EDGE_CAP_MILLI=800000
#
# The model coefficient used for the prediction check, set to the measured mean.
MODEL_EDGE_BYTES=410
# The model held to within 0.5% at this shape and 3.4% across an 8x scale sweep.
# +25% is far outside that, so a trip is a real change in cost per edge. The
# lower bound is deliberate and symmetric: a measurement well under prediction
# means the model no longer describes the code, and a gate calibrated on a dead
# model is a gate that cannot fire. Both directions demand re-derivation, which
# is the SC27 discipline — a constant that no longer matches reality is a defect
# even when it errs generously.
PRED_MAX_PCT=125
PRED_MIN_PCT=60
#
# Wall-clock bound so a pathological regression cannot hang CI instead of
# failing it. Two builds of this corpus measure ~2.4 s each on a quiet
# workstation; 240 s leaves ample room for a loaded 2-core runner.
TIME_BUDGET_S=240
# -----------------------------------------------------------------------------

for pair in "DEFS=$DEFS" "CALLERS=$CALLERS" "FNS=$FNS" "CALLEES=$CALLEES"; do
  [[ "${pair#*=}" =~ ^[1-9][0-9]*$ ]] || {
    echo "PROBE FAIL: $pair is not a positive integer; the probe's expected fan-out is computed from it"
    exit 1; }
done
# The resolver caps how many edges one ambiguous site may emit, so the width
# this corpus *gets* is not the width it declares.
#
# `AMBIGUOUS_FANOUT_CAP` landed with audit R-7, after this probe was written,
# and every arithmetic precondition below was stated in terms of `DEFS`. With
# DEFS=100 and a cap of 16 the probe asserts a widest fan-out of 100, gets 16,
# and fails — which nobody saw, because `verify.sh` invoked this script as
# `./tools/memory_model_probe.sh` and it was committed non-executable, so step 6
# never ran at all.
#
# The constant is read from its owner rather than repeated, the same way
# `verify.sh` reads `DB_SIZE_GATE_PER_FILE`, and a missing constant fails closed:
# a probe that silently fell back to `DEFS` would be asserting the shape of a
# resolver that no longer exists.
FANOUT_CAP=$(grep -oE 'AMBIGUOUS_FANOUT_CAP: usize = [0-9]+' \
  crates/devmap-resolve/src/model.rs | grep -oE '[0-9]+$')
[[ "$FANOUT_CAP" =~ ^[1-9][0-9]*$ ]] || {
  echo "PROBE FAIL: cannot read AMBIGUOUS_FANOUT_CAP from crates/devmap-resolve/src/model.rs"
  exit 1; }
# What one site actually emits: the corpus width, bounded by the cap.
WIDTH=$DEFS
[ "$WIDTH" -le "$FANOUT_CAP" ] || WIDTH=$FANOUT_CAP

# Asked of the *effective* width, not of `DEFS`. The bytes-per-pair bound is
# bytes-per-edge divided by the mean fan-out, so it is the width the resolver
# emits that decides whether the bound is meaningful — raising `DEFS` past the
# cap buys no width and would leave this check passing on a corpus it no longer
# describes.
[ "$WIDTH" -ge "$MIN_DEFS" ] || {
  echo "PROBE FAIL: effective fan-out width $WIDTH (DEFS=$DEFS capped at $FANOUT_CAP) is below the minimum width $MIN_DEFS the bytes-per-pair bound is valid for"
  exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Emit one corpus. `mode=ambiguous` gives every def file the same names;
# `mode=unique` gives each def file its own, so the two corpora have identical
# file, symbol and call counts and differ only in candidate multiplicity.
generate() { # <dir> <mode>
  local dir=$1 mode=$2 k=0 c=0 j p
  mkdir -p "$dir/defs" "$dir/callers"
  while [ "$k" -lt "$DEFS" ]; do
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

FILES=$((DEFS + CALLERS))
SITES=$((CALLERS * FNS * CALLEES))
EXPECT_EDGES=$((SITES * WIDTH))
EXPECT_SUM_N2=$((SITES * WIDTH * WIDTH))

echo "CAPPED: this probe builds a synthetic ${FILES}-file corpus (Sum(N^2) = ${EXPECT_SUM_N2}, widest fan-out ${WIDTH}"
if [ "$WIDTH" -lt "$DEFS" ]; then
  echo "CAPPED: — ${DEFS} candidates per name, bounded by AMBIGUOUS_FANOUT_CAP=${FANOUT_CAP})."
else
  echo "CAPPED: )."
fi
echo "CAPPED: the production corpus is 12,831 files and is NOT built here. This bounds the per-pair and"
echo "CAPPED: per-edge memory coefficients, which are corpus-size invariant; it does not bound any real"
echo "CAPPED: repository's absolute peak. Raise DEVMAP_PROBE_CALLERS to scale the probe up locally."

START=$(date +%s)
generate "$TMP/amb" ambiguous
generate "$TMP/ctl" unique

RSS_AMB=$(peak_rss_bytes "$TMP/amb.time" "$DEVMAP" --db "$TMP/amb.sqlite" --progress never build "$TMP/amb") || {
  echo "PROBE FAIL: could not measure peak RSS for the ambiguous build — refusing to report an unmeasured probe as passing"; exit 1; }
RSS_CTL=$(peak_rss_bytes "$TMP/ctl.time" "$DEVMAP" --db "$TMP/ctl.sqlite" --progress never build "$TMP/ctl") || {
  echo "PROBE FAIL: could not measure peak RSS for the control build — refusing to report an unmeasured probe as passing"; exit 1; }
ELAPSED=$(( $(date +%s) - START ))

M_AMB=$(tools/fanout.sh "$TMP/amb.sqlite") || { echo "PROBE FAIL: fan-out metrics unavailable for the ambiguous build"; exit 1; }
M_CTL=$(tools/fanout.sh "$TMP/ctl.sqlite") || { echo "PROBE FAIL: fan-out metrics unavailable for the control build"; exit 1; }

SUM_N2=$(field "$M_AMB" fanout_sum_n2)
EDGES=$(field "$M_AMB" fanout_edges)
GOT_SITES=$(field "$M_AMB" fanout_sites)
GOT_MAX=$(field "$M_AMB" fanout_max_n)
CTL_SUM_N2=$(field "$M_CTL" fanout_sum_n2)

echo "probe: ${FILES} files, ${GOT_SITES} ambiguous sites, widest ${GOT_MAX}, Sum(N)=${EDGES}, Sum(N^2)=${SUM_N2}"
echo "probe: peak RSS ambiguous $((RSS_AMB / 1024 / 1024)) MiB, control $((RSS_CTL / 1024 / 1024)) MiB, ${ELAPSED}s"

# The corpus shape is known exactly, so the derived metric is checked against
# arithmetic before it is used as the denominator of anything. This is the same
# validation `test_fanout_metric.rs` does on a hand-counted fixture, repeated
# here at 10^7 scale: a grouping key that were merely plausible would still have
# to reproduce SITES x DEFS^2 exactly.
[ "$GOT_SITES" -eq "$SITES" ] || { echo "PROBE FAIL: derived $GOT_SITES ambiguous sites, corpus has $SITES"; exit 1; }
[ "$GOT_MAX" -eq "$WIDTH" ] || { echo "PROBE FAIL: derived widest fan-out $GOT_MAX, corpus emits $WIDTH (DEFS=$DEFS, AMBIGUOUS_FANOUT_CAP=$FANOUT_CAP)"; exit 1; }
[ "$EDGES" -eq "$EXPECT_EDGES" ] || { echo "PROBE FAIL: derived Sum(N)=$EDGES, corpus has $EXPECT_EDGES"; exit 1; }
[ "$SUM_N2" -eq "$EXPECT_SUM_N2" ] || { echo "PROBE FAIL: derived Sum(N^2)=$SUM_N2, corpus has $EXPECT_SUM_N2"; exit 1; }
# The control must contain no ambiguity at all, or it is not a base measurement
# and the subtraction below is meaningless.
[ "$CTL_SUM_N2" -eq 0 ] || { echo "PROBE FAIL: the control corpus has fan-out (Sum(N^2)=$CTL_SUM_N2); it cannot serve as the base term"; exit 1; }

[ "$RSS_AMB" -gt "$RSS_CTL" ] || {
  echo "PROBE FAIL: the ambiguous build did not cost more than its control ($RSS_AMB vs $RSS_CTL) — the fan-out is not being measured"; exit 1; }
DELTA=$((RSS_AMB - RSS_CTL))

PAIR_MILLI=$((DELTA * 1000 / SUM_N2))
EDGE_MILLI=$((DELTA * 1000 / EDGES))
PREDICTED=$((RSS_CTL + MODEL_EDGE_BYTES * EDGES))
PRED_PCT=$((RSS_AMB * 100 / PREDICTED))
# What the ledger's pre-Arc model would have predicted, printed for comparison
# rather than gated: 112 B per candidate pair.
SC3_PREDICTED=$((RSS_CTL + 112 * SUM_N2))

echo "probe: fan-out cost $((DELTA / 1024 / 1024)) MiB => ${PAIR_MILLI} milli-bytes/pair (cap ${PAIR_CAP_MILLI}), ${EDGE_MILLI} milli-bytes/edge (cap ${EDGE_CAP_MILLI})"
echo "probe: predicted $((PREDICTED / 1024 / 1024)) MiB from base + ${MODEL_EDGE_BYTES} B x Sum(N); measured is ${PRED_PCT}% of that"
echo "probe: for comparison, the pre-SC3 model (base + 112 B x Sum(N^2)) predicts $((SC3_PREDICTED / 1024 / 1024)) MiB"

FAIL=0
[ "$PAIR_MILLI" -lt "$PAIR_CAP_MILLI" ] || {
  echo "PROBE FAIL: ${PAIR_MILLI} milli-bytes per candidate pair >= cap ${PAIR_CAP_MILLI}"; FAIL=1; }
[ "$EDGE_MILLI" -lt "$EDGE_CAP_MILLI" ] || {
  echo "PROBE FAIL: ${EDGE_MILLI} milli-bytes per fan-out edge >= cap ${EDGE_CAP_MILLI}"; FAIL=1; }
[ "$PRED_PCT" -le "$PRED_MAX_PCT" ] || {
  echo "PROBE FAIL: measured peak is ${PRED_PCT}% of the model's prediction (max ${PRED_MAX_PCT}%)"; FAIL=1; }
[ "$PRED_PCT" -ge "$PRED_MIN_PCT" ] || {
  echo "PROBE FAIL: measured peak is ${PRED_PCT}% of the model's prediction (min ${PRED_MIN_PCT}%) — the model no longer describes the code and must be re-derived, not left in place"; FAIL=1; }
[ "$ELAPSED" -le "$TIME_BUDGET_S" ] || {
  echo "PROBE FAIL: probe took ${ELAPSED}s, budget ${TIME_BUDGET_S}s"; FAIL=1; }

[ "$FAIL" -eq 0 ] || exit 1
echo "MEMORY MODEL OK (${PAIR_MILLI} milli-B/pair, ${EDGE_MILLI} milli-B/edge, ${PRED_PCT}% of prediction)"
