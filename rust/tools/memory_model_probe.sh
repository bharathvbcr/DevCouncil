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
# `rust/target/release/devmap` would report the previous kernel's numbers
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
# WHAT THE DENOMINATOR IS, AND WHAT IT WAS
# ----------------------------------------
# This block used to gate two coefficients — bytes per candidate *pair* (the
# Sum(N^2) term) and bytes per emitted *edge* — and both were red. Neither was
# wrong by a little: both had the wrong denominator.
#
# `AMBIGUOUS_FANOUT_CAP` (audit R-7) bounds how many edges one ambiguous site
# emits, at 16. It does not bound the site's candidate list, which the
# `Arc<Resolution>` still holds in full — deliberately, because that list is
# what keeps `impact` answerable on candidates 2..N. So since R-7 resolver
# memory has been proportional to *candidates* while every number this probe
# could derive counted *emitted edges*, and the two stopped being the same
# quantity. Schema v16 puts `generation_edges.candidate_total` in the store and
# `tools/fanout.sh` now reports it, which is what made the re-derivation below
# possible: the fix was a store change, not a coefficient change.
#
# THE MEASUREMENT (2026-09-06, this machine, `devmap` release build)
# -----------------------------------------------------------------
# Seven shapes, sweeping fan-out width 16..200 (12.5x) at a fixed 80,000 edges
# and site count 100..400 (4x) at a fixed width, each an ambiguous corpus minus
# its own unique-name control:
#
#   DEFS  CALLERS    edges  candidates   delta bytes   B/edge   B/candidate
#     16      100    80000       80000      59883520    748.5         748.5
#     32      100    80000      160000      71598080    895.0         447.5
#     64      100    80000      320000      86081536   1076.0         269.0
#    100      100    80000      500000     100483072   1256.0         201.0
#    200      100    80000     1000000     141819904   1772.7         141.8
#    100      200   160000     1000000     196771840   1229.8         196.8
#    100      400   320000     2000000     391495680   1223.4         195.7
#
# Neither single-term coefficient is scale-invariant. Bytes per emitted edge
# moves 2.4x across the width sweep; bytes per candidate *pair* — the old
# `PAIR_CAP_MILLI` denominator — moves **66x**, from 46,784 to 709 milli-B/pair.
# A gate on a number that varies 66x with corpus shape is not a bound, and the
# reason it passed for so long is that it sat ten times under its cap at the one
# shape it was calibrated on.
#
# A two-term model fits all seven points. Least squares with no intercept (the
# control *is* the base, measured rather than modelled):
#
#     delta = 694 B x emitted_edges + 86 B x candidates
#
# Measured against predicted: 96.1, 103.5, 103.9, 102.2, 100.5, 100.1, 99.6 —
# a 96.1%..103.9% band across a 12.5x width range and a 4x site range. That is
# the scale invariance this probe exists to assert, and it is now asserted on a
# model that describes the code rather than one that predates a cap.
MODEL_EDGE_BYTES=694
MODEL_CANDIDATE_BYTES=86
# Measured band is 96.1..103.9; +/-11 points of headroom on each side. Tighter
# than the 60..125 it replaces *and* better founded — the old band was wide
# because the model underneath it was wrong, not because the code is noisy.
#
# The lower bound is deliberate and symmetric, unchanged in spirit from what it
# replaces: a measurement well under prediction means the model no longer
# describes the code, and a gate calibrated on a dead model cannot fire. Both
# directions demand re-derivation. That discipline is what produced this block.
PRED_MAX_PCT=115
PRED_MIN_PCT=85
#
# BYTES PER CANDIDATE, the residual after the edge term. This is the `Arc`
# guard, and it is what the retired pair cap was reaching for. SC3 measured the
# pre-`Arc` resolver at 112-128 B per candidate *pair*, because each of a site's
# N edges owned its own clone of the N-element candidate list: cost was
# quadratic in width. With the list shared it is linear, and the residual
# measures 55.0, 100.7, 95.6, 90.0, 86.3, 85.8, 84.8 B/candidate across the
# sweep — flat. 150 is 1.5x the measured maximum. A revert of the `Arc` fix
# makes this grow *with width*: at the default shape a cloned list costs 100x
# more per candidate, so the cap is not close to noise in the direction that
# matters.
#
# (The 55.0 at DEFS=16 is the degenerate case where the cap is inert and
# edges == candidates, so the two terms are collinear and the split between
# them is arbitrary. It is below the cap and is not the case the cap guards.)
CANDIDATE_CAP_MILLI=150000
#
# WIDTH INVARIANCE, the direct test of the shared-`Arc` invariant.
#
# The cap above bounds the coefficient at one shape. This bounds how it *moves*:
# the probe measures a second, wider corpus and requires bytes-per-candidate not
# to grow. Under a clone revert it grows linearly with width — doubling the
# width doubles it — which no single-shape cap can distinguish from a corpus
# that simply got bigger. Measured across the width sweep the ratio falls
# (100.7 -> 86.3 from width 32 to 200), so a ceiling of 125% is a real bound
# rather than a restatement of the cap.
WIDTH_RATIO_MAX_PCT=125
WIDE_DEFS_MULTIPLIER=2
#
# The probe's primary shape must separate the two terms, which needs the fan-out
# cap to be *active*: at a width at or under the cap every site emits one edge
# per candidate, the two denominators coincide, and the model's split between
# them is unidentifiable. `MIN_CANDIDATE_RATIO` demands at least four candidates
# per emitted edge — the default shape gives 6.25.
MIN_CANDIDATE_RATIO=4
#
# Wall-clock bound so a pathological regression cannot hang CI instead of
# failing it. Four builds now rather than two — the wide corpus is a second
# ambiguous/control pair — measuring ~2.4 s each on a quiet workstation. 480 s
# leaves the same proportional room on a loaded 2-core runner that 240 s left
# for two builds.
TIME_BUDGET_S=480
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
  devmap-resolve/src/model.rs | grep -oE '[0-9]+$')
[[ "$FANOUT_CAP" =~ ^[1-9][0-9]*$ ]] || {
  echo "PROBE FAIL: cannot read AMBIGUOUS_FANOUT_CAP from devmap-resolve/src/model.rs"
  exit 1; }
# What one site actually emits: the corpus width, bounded by the cap.
WIDTH=$DEFS
[ "$WIDTH" -le "$FANOUT_CAP" ] || WIDTH=$FANOUT_CAP

# The two model terms have to be separable, and they are only separable when the
# cap is *active*: at `DEFS <= FANOUT_CAP` every site emits one edge per
# candidate, the two denominators are the same number, and the model's split
# between them is unidentifiable. Asked of the ratio rather than of a width, so
# the check stays correct if `AMBIGUOUS_FANOUT_CAP` moves.
CANDIDATE_RATIO=$((DEFS / WIDTH))
[ "$CANDIDATE_RATIO" -ge "$MIN_CANDIDATE_RATIO" ] || {
  echo "PROBE FAIL: ${CANDIDATE_RATIO} candidates per emitted edge (DEFS=$DEFS, AMBIGUOUS_FANOUT_CAP=$FANOUT_CAP) is under the minimum $MIN_CANDIDATE_RATIO; the edge and candidate terms of the model are not separable at this shape"
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
EXPECT_CANDIDATES=$((SITES * DEFS))

echo "CAPPED: this probe builds a synthetic ${FILES}-file corpus: ${SITES} ambiguous sites, ${DEFS} candidates"
echo "CAPPED: each, emitting ${WIDTH} edges each (AMBIGUOUS_FANOUT_CAP=${FANOUT_CAP}); ${EXPECT_CANDIDATES} candidates"
echo "CAPPED: weighed against ${EXPECT_EDGES} edges emitted. A second, ${WIDE_DEFS_MULTIPLIER}x wider corpus is built"
echo "CAPPED: for the width-invariance check. The production corpus is 12,831 files and is NOT built here:"
echo "CAPPED: this bounds two corpus-size-invariant coefficients and the model that combines them; it does"
echo "CAPPED: not bound any real repository's absolute peak. Raise DEVMAP_PROBE_CALLERS to scale up locally."

START=$(date +%s)

# One ambiguous/control pair at a given candidate width.
#
# Echoes `<edges> <candidates> <delta_bytes> <sum_n2> <ctl_sum_n2> <sites> <max_n>`.
# The control *is* the base term, measured rather than modelled: a coefficient
# computed against a modelled base inherits that model's error, and on a real
# repository the base is ~97% of peak.
measure_pair() { # <defs> <tag>
  local defs=$1 tag=$2 rss_amb rss_ctl m_amb m_ctl
  DEFS=$defs generate "$TMP/$tag.amb" ambiguous
  DEFS=$defs generate "$TMP/$tag.ctl" unique
  rss_amb=$(peak_rss_bytes "$TMP/$tag.amb.time" "$DEVMAP" --db "$TMP/$tag.amb.sqlite" --progress never build "$TMP/$tag.amb") || {
    echo "PROBE FAIL: could not measure peak RSS for the $tag ambiguous build — refusing to report an unmeasured probe as passing" >&2; return 1; }
  rss_ctl=$(peak_rss_bytes "$TMP/$tag.ctl.time" "$DEVMAP" --db "$TMP/$tag.ctl.sqlite" --progress never build "$TMP/$tag.ctl") || {
    echo "PROBE FAIL: could not measure peak RSS for the $tag control build — refusing to report an unmeasured probe as passing" >&2; return 1; }
  m_amb=$(tools/fanout.sh "$TMP/$tag.amb.sqlite") || {
    echo "PROBE FAIL: fan-out metrics unavailable for the $tag ambiguous build" >&2; return 1; }
  m_ctl=$(tools/fanout.sh "$TMP/$tag.ctl.sqlite") || {
    echo "PROBE FAIL: fan-out metrics unavailable for the $tag control build" >&2; return 1; }
  [ "$rss_amb" -gt "$rss_ctl" ] || {
    echo "PROBE FAIL: the $tag ambiguous build did not cost more than its control ($rss_amb vs $rss_ctl) — the fan-out is not being measured" >&2; return 1; }
  printf '%s %s %s %s %s %s %s %s\n' \
    "$(field "$m_amb" fanout_edges)" \
    "$(field "$m_amb" candidates)" \
    "$((rss_amb - rss_ctl))" \
    "$(field "$m_amb" fanout_sum_n2)" \
    "$(field "$m_ctl" fanout_sum_n2)" \
    "$(field "$m_amb" fanout_sites)" \
    "$(field "$m_amb" fanout_max_n)" \
    "$(field "$m_amb" max_candidates)"
}

read -r EDGES CANDIDATES DELTA SUM_N2 CTL_SUM_N2 GOT_SITES GOT_MAX GOT_MAX_CAND \
  < <(measure_pair "$DEFS" narrow) || exit 1

WIDE_DEFS=$((DEFS * WIDE_DEFS_MULTIPLIER))
read -r W_EDGES W_CANDIDATES W_DELTA _ _ _ _ _ < <(measure_pair "$WIDE_DEFS" wide) || exit 1

ELAPSED=$(( $(date +%s) - START ))

echo "probe: ${FILES} files, ${GOT_SITES} ambiguous sites, widest fan-out ${GOT_MAX}, widest candidate list ${GOT_MAX_CAND}"
echo "probe: Sum(N)=${EDGES} edges, ${CANDIDATES} candidates weighed, Sum(N^2)=${SUM_N2}"
echo "probe: fan-out cost $((DELTA / 1024 / 1024)) MiB narrow, $((W_DELTA / 1024 / 1024)) MiB at ${WIDE_DEFS} candidates, ${ELAPSED}s"

# The corpus shape is known exactly, so every derived metric is checked against
# arithmetic before it is used as the denominator of anything. This is the same
# validation `test_fanout_metric.rs` does on a hand-counted fixture, repeated
# here at 10^6 scale: a grouping key that were merely plausible would still have
# to reproduce SITES x WIDTH and SITES x DEFS exactly.
[ "$GOT_SITES" -eq "$SITES" ] || { echo "PROBE FAIL: derived $GOT_SITES ambiguous sites, corpus has $SITES"; exit 1; }
[ "$GOT_MAX" -eq "$WIDTH" ] || { echo "PROBE FAIL: derived widest fan-out $GOT_MAX, corpus emits $WIDTH (DEFS=$DEFS, AMBIGUOUS_FANOUT_CAP=$FANOUT_CAP)"; exit 1; }
[ "$GOT_MAX_CAND" -eq "$DEFS" ] || { echo "PROBE FAIL: derived widest candidate list $GOT_MAX_CAND, corpus declares $DEFS — the candidate denominator is not what this probe thinks it is"; exit 1; }
[ "$EDGES" -eq "$EXPECT_EDGES" ] || { echo "PROBE FAIL: derived Sum(N)=$EDGES, corpus has $EXPECT_EDGES"; exit 1; }
[ "$CANDIDATES" -eq "$EXPECT_CANDIDATES" ] || { echo "PROBE FAIL: derived $CANDIDATES candidates, corpus has $EXPECT_CANDIDATES"; exit 1; }
[ "$SUM_N2" -eq "$EXPECT_SUM_N2" ] || { echo "PROBE FAIL: derived Sum(N^2)=$SUM_N2, corpus has $EXPECT_SUM_N2"; exit 1; }
# The control must contain no ambiguity at all, or it is not a base measurement
# and the subtraction is meaningless.
[ "$CTL_SUM_N2" -eq 0 ] || { echo "PROBE FAIL: the control corpus has fan-out (Sum(N^2)=$CTL_SUM_N2); it cannot serve as the base term"; exit 1; }
# The wide corpus must actually be wider in the dimension under test, or the
# invariance check compares a shape against itself.
[ "$W_CANDIDATES" -gt "$CANDIDATES" ] || {
  echo "PROBE FAIL: the wide corpus weighed $W_CANDIDATES candidates against the narrow corpus's $CANDIDATES; the width-invariance check has nothing to compare"; exit 1; }

# The model, on the *delta* rather than on the total. The base term is in both
# sides of a total-vs-total ratio and dilutes it — on this shape a 2x error in
# the fan-out cost would move a total ratio by well under the band. The delta is
# what the coefficients describe and what they were measured against.
MODELLED=$((MODEL_EDGE_BYTES * EDGES + MODEL_CANDIDATE_BYTES * CANDIDATES))
PRED_PCT=$((DELTA * 100 / MODELLED))
# The residual after the edge term, per candidate. Clamped at zero: a negative
# residual means the edge term alone already over-predicts, which the model band
# above catches with a message that says so.
EDGE_PART=$((MODEL_EDGE_BYTES * EDGES))
CAND_RESIDUAL=$((DELTA - EDGE_PART))
[ "$CAND_RESIDUAL" -gt 0 ] || CAND_RESIDUAL=0
CAND_MILLI=$((CAND_RESIDUAL * 1000 / CANDIDATES))
W_CAND_RESIDUAL=$((W_DELTA - MODEL_EDGE_BYTES * W_EDGES))
[ "$W_CAND_RESIDUAL" -gt 0 ] || W_CAND_RESIDUAL=0
W_CAND_MILLI=$((W_CAND_RESIDUAL * 1000 / W_CANDIDATES))
if [ "$CAND_MILLI" -gt 0 ]; then
  WIDTH_RATIO_PCT=$((W_CAND_MILLI * 100 / CAND_MILLI))
else
  WIDTH_RATIO_PCT=0
fi

# Retired denominators, printed rather than gated. Bytes per emitted edge moves
# 2.4x across a width sweep and bytes per candidate *pair* moves 66x, so neither
# is a bound; they are here because three years of this ledger quote them and a
# reader comparing runs needs the same numbers to compare.
EDGE_MILLI=$((DELTA * 1000 / EDGES))
PAIR_MILLI=$((DELTA * 1000 / SUM_N2))

echo "probe: ${CAND_MILLI} milli-bytes/candidate (cap ${CANDIDATE_CAP_MILLI}), ${W_CAND_MILLI} at ${WIDE_DEFS} candidates => ${WIDTH_RATIO_PCT}% (max ${WIDTH_RATIO_MAX_PCT}%)"
echo "probe: model predicts $((MODELLED / 1024 / 1024)) MiB from ${MODEL_EDGE_BYTES} B x ${EDGES} edges + ${MODEL_CANDIDATE_BYTES} B x ${CANDIDATES} candidates; measured is ${PRED_PCT}%"
echo "probe: not gated, for continuity with older runs — ${EDGE_MILLI} milli-B/edge, ${PAIR_MILLI} milli-B/pair"
echo "probe: for comparison, the pre-SC3 model (112 B x Sum(N^2)) predicts $(((112 * SUM_N2) / 1024 / 1024)) MiB of fan-out cost"

FAIL=0
[ "$CAND_MILLI" -lt "$CANDIDATE_CAP_MILLI" ] || {
  echo "PROBE FAIL: ${CAND_MILLI} milli-bytes per candidate >= cap ${CANDIDATE_CAP_MILLI} — the candidate list is costing per-edge memory again, which is what sharing it through an Arc was for"; FAIL=1; }
[ "$WIDTH_RATIO_PCT" -le "$WIDTH_RATIO_MAX_PCT" ] || {
  echo "PROBE FAIL: bytes per candidate grew to ${WIDTH_RATIO_PCT}% when the candidate list doubled (max ${WIDTH_RATIO_MAX_PCT}%) — cost is scaling with width, so the list is no longer shared"; FAIL=1; }
[ "$PRED_PCT" -le "$PRED_MAX_PCT" ] || {
  echo "PROBE FAIL: measured fan-out cost is ${PRED_PCT}% of the model's prediction (max ${PRED_MAX_PCT}%)"; FAIL=1; }
[ "$PRED_PCT" -ge "$PRED_MIN_PCT" ] || {
  echo "PROBE FAIL: measured fan-out cost is ${PRED_PCT}% of the model's prediction (min ${PRED_MIN_PCT}%) — the model no longer describes the code and must be re-derived, not left in place"; FAIL=1; }
[ "$ELAPSED" -le "$TIME_BUDGET_S" ] || {
  echo "PROBE FAIL: probe took ${ELAPSED}s, budget ${TIME_BUDGET_S}s"; FAIL=1; }

[ "$FAIL" -eq 0 ] || exit 1
echo "MEMORY MODEL OK (${CAND_MILLI} milli-B/candidate, ${WIDTH_RATIO_PCT}% width ratio, ${PRED_PCT}% of prediction)"
