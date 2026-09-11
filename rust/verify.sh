#!/usr/bin/env bash
# devmap verification pipeline — run from rust/ on a machine with a Rust toolchain.
# Usage: ./verify.sh [--quick] [--mutants]
set -euo pipefail
cd "$(dirname "$0")"

QUICK=0; MUTANTS=0
for a in "$@"; do
  case "$a" in
    --quick) QUICK=1 ;;
    --mutants) MUTANTS=1 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done

step() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
now_ns() {
  local value
  value=$(date +%s%N 2>/dev/null || true)
  if [[ "$value" =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$value"
  elif command -v gdate >/dev/null 2>&1; then
    gdate +%s%N
  elif command -v perl >/dev/null 2>&1; then
    perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9'
  else
    echo "no nanosecond clock available (need GNU date, gdate, or perl)" >&2
    return 1
  fi
}

step "1/9 format"
cargo fmt --all -- --check

step "2/9 clippy (-D warnings)"
cargo clippy --workspace --all-targets -- -D warnings

step "3/9 tests"
cargo test --workspace

if [ "$QUICK" -eq 1 ]; then echo "quick mode: skipping determinism/perf/mutants"; exit 0; fi

step "release worker panic recovery (executable, not test profile)"
cargo run --release --locked -q -p devmap-serve --example worker_recovery_probe

# Where `cargo` actually put the release binary. Steps 5 and 8 run it directly
# rather than through `cargo run`, so they have to agree with cargo about the
# path — and `CARGO_TARGET_DIR` is set by anyone running two lanes against one
# checkout, which is a layout this repository's own guidance recommends. Before
# this, such a run reported "could not measure peak RSS", which is true and is
# the wrong reason: the measurement was fine and the binary was not there.
DEVMAP_BIN="${CARGO_TARGET_DIR:-$(pwd)/target}/release/devmap"
# Exported so `tools/memory_model_probe.sh` and `tools/soak.sh` measure the same
# binary these gates did, rather than each resolving it again.
export DEVMAP_BIN

step "4/9 determinism — two clean builds must produce identical graph digests"
TMP1=$(mktemp -d) ; TMP2=$(mktemp -d)
trap 'rm -rf "$TMP1" "$TMP2"' EXIT
cargo run --release -q -p devmap-cli -- --db "$TMP1/a.sqlite" build ./testdata >/dev/null
cargo run --release -q -p devmap-cli -- --db "$TMP2/b.sqlite" build ./testdata >/dev/null
DIGEST_SQL="SELECT group_concat(record, char(10)) FROM (
  SELECT source_path.path || '>' || target_path.path || ':' ||
         edge.source_symbol || '>' || edge.target_symbol || ':' ||
         edge.edge_kind || ':' || printf('%.17g', edge.confidence) AS record
  FROM generation_edges AS edge
  JOIN paths AS source_path ON source_path.id = edge.source_file_id
  JOIN paths AS target_path ON target_path.id = edge.target_file_id
  WHERE edge.generation_id = (SELECT max(id) FROM generations)
  ORDER BY source_path.path, target_path.path, edge.source_symbol,
           edge.target_symbol, edge.edge_kind, edge.confidence
)"
D1=$(sqlite3 "$TMP1/a.sqlite" "$DIGEST_SQL" | shasum -a 256 | cut -d' ' -f1)
D2=$(sqlite3 "$TMP2/b.sqlite" "$DIGEST_SQL" | shasum -a 256 | cut -d' ' -f1)
[ "$D1" = "$D2" ] && echo "determinism OK ($D1)" || { echo "DETERMINISM FAILURE: $D1 != $D2"; exit 1; }

step "5/9 self-build gates — DevCouncil repo, release"
# Peak RSS is measured, not assumed, and an unavailable reading fails this step
# rather than skipping it. The measurement itself lives in tools/peak_rss.sh so
# this gate and the memory-model probe in step 6 share one implementation.
# shellcheck source=tools/peak_rss.sh
. tools/peak_rss.sh

START=$(now_ns)
[ -x "$DEVMAP_BIN" ] || {
  echo "GATE FAIL: no release binary at $DEVMAP_BIN — step 4 builds it with \`cargo run --release\`, so this means cargo wrote it somewhere else (check CARGO_TARGET_DIR)"; exit 1; }
RSS=$(PEAK_RSS_STDOUT_FILE="$TMP1/self.json" peak_rss_bytes "$TMP1/self.rss" "$DEVMAP_BIN" --db "$TMP1/self.sqlite" --json --progress never build ..) || {
  echo "GATE FAIL: could not measure peak RSS — refusing to report an unmeasured build as passing"; exit 1; }
END=$(now_ns)
MS=$(( (END - START) / 1000000 ))
perl -MJSON::PP -e '
  open my $fh, "<", $ARGV[0] or die $!;
  my $raw = do { local $/; <$fh> };
  my $report = decode_json($raw);
  die "RSS report has no timings object\n" unless ref($report->{timings}) eq "HASH";
  print "measured build stages: ", encode_json($report->{timings}), "\n";
' "$TMP1/self.json"
# This measures a COLD build, and is compared against the per-generation budget.
# The steady state — what a running repository actually sits at — is gated in
# the growth step instead (SC27).
BYTES=$(stat -f%z "$TMP1/self.sqlite" 2>/dev/null || stat -c%s "$TMP1/self.sqlite")
FILE_COUNT=$(sqlite3 "$TMP1/self.sqlite" "SELECT COUNT(*) FROM generation_files WHERE generation_id = (SELECT max(id) FROM generations)")

# Size budget, derived from the Rust constant rather than repeated here.
#
# These drifted once already: `db_size_gate_bytes` still said 80 KiB/file after
# this gate was recalibrated to 160 KiB, and nothing caught it because no
# production code called that function (SC15). Reading the constant keeps one
# owner for the policy.
PER_FILE_KIB=$(grep -oE 'DB_SIZE_GATE_PER_FILE: u64 = [0-9]+' \
  devmap-extract/src/model.rs | grep -oE '[0-9]+$')
[ -n "$PER_FILE_KIB" ] || { echo "GATE FAIL: cannot read DB_SIZE_GATE_PER_FILE"; exit 1; }
GATE=$(( 60*1024*1024 ))
SCALED=$(( FILE_COUNT * PER_FILE_KIB * 1024 ))
if [ "$SCALED" -gt "$GATE" ]; then GATE=$SCALED; fi
# 2 GiB: measured peak after the Arc fix is 1.48 GiB on a 4.7k-file repository
# and ~0.35 GiB here, so this catches a regression toward the old behaviour
# long before it reaches a CI runner's limit.
#
# This bound is ABSOLUTE, and that is its limitation rather than a defect: it is
# calibrated on a 1.1k-file repository, and the corpus this port is run against
# is 12,831 files, which legitimately uses more. It is kept, and a
# scale-invariant budget is added below — both, not either.
RSS_GATE=$(( 2 * 1024*1024*1024 ))

# Scale-invariant memory budget.
#
# Peak resident memory has two terms, and until now nothing bounded either as a
# *rate*, so no gate said anything at all about a corpus larger than this one.
# Measured on this repository 2026-08-17: peak 309 MiB over 1,114 files (and
# 316 MiB over 1,121 an hour later — the tree moves), of which the fan-out term
# is 21,963 x 410 B = 8.6 MiB, leaving ~283 KiB/file of base. The
# 12,831-file personal corpus at 3.40-3.55 GiB implies 278 KiB/file — the same
# rate at 11x the size, which is what makes a per-file budget meaningful here.
#
# 512 KiB/file is 1.8x the measured rate. SC3's regime was 10.45 GiB over 4,742
# files = 2.31 MiB/file, 4.5x over this budget, so the gate that did not exist
# then would have been red. The headroom also absorbs allocator differences
# between the macOS host these numbers were measured on and the Linux runner CI
# uses, which has not been measured.
#
# The fan-out term has TWO coefficients, not one, and for a while it had the
# wrong one.
#
# `AMBIGUOUS_FANOUT_CAP` bounds how many edges an ambiguous site emits (16); it
# does not bound the candidate list the `Arc<Resolution>` holds, which is what
# the memory is proportional to. A single 600 B/edge term was therefore charging
# the wrong quantity — on a corpus whose ambiguous sites weigh many candidates
# each it under-counts by the ratio between the two. Measured on this repository
# 2026-09-06: 59,309 candidates weighed against 38,006 edges emitted, 1.56x, and
# a corpus with wider ambiguity separates them further. Re-derived by
# `tools/memory_model_probe.sh` over a seven-shape sweep (fan-out width 16..200,
# site count 100..400):
#
#     fan-out cost = 694 B x emitted_edges + 86 B x candidates
#
# fitting every point to 96.1%..103.9%. Both terms are carried here, each with
# the same 1.5x headroom the single term had: 1050 and 130. `candidates` comes
# from `generation_edges.candidate_total` (schema v16), which is what made the
# right denominator derivable from a store at all.
RSS_BASE_PER_FILE=$(( 512 * 1024 ))
RSS_PER_FANOUT_EDGE=1050
RSS_PER_CANDIDATE=130

# The fan-out is derived from the persisted store, not from the resolver's
# source, and it fails closed: tools/fanout.sh exits non-zero if any fan-out
# edge cannot be joined to a node or if its two independent recoveries of the
# callee name disagree. A budget whose denominator could not be derived must not
# be reported as a budget that was met.
FANOUT=$(./tools/fanout.sh "$TMP1/self.sqlite") || {
  echo "GATE FAIL: could not derive the ambiguity fan-out — refusing to report an underived memory budget as passing"; exit 1; }
FANOUT_EDGES=$(printf '%s\n' "$FANOUT" | tr ' ' '\n' | awk -F= '$1=="fanout_edges" {print $2; exit}')
FANOUT_CANDIDATES=$(printf '%s\n' "$FANOUT" | tr ' ' '\n' | awk -F= '$1=="candidates" {print $2; exit}')
[[ "$FANOUT_EDGES" =~ ^[0-9]+$ ]] || { echo "GATE FAIL: malformed fan-out metrics: $FANOUT"; exit 1; }
[[ "$FANOUT_CANDIDATES" =~ ^[0-9]+$ ]] || { echo "GATE FAIL: fan-out metrics carry no candidate total: $FANOUT"; exit 1; }
RSS_BUDGET=$(( FILE_COUNT * RSS_BASE_PER_FILE \
             + FANOUT_EDGES * RSS_PER_FANOUT_EDGE \
             + FANOUT_CANDIDATES * RSS_PER_CANDIDATE ))

echo "build ${MS} ms, db $((BYTES/1024/1024)) MiB (cold), files ${FILE_COUNT}, gate $((GATE/1024/1024)) MiB, peak RSS $((RSS/1024/1024)) MiB"
echo "$FANOUT"
echo "rss budget $((RSS_BUDGET/1024/1024)) MiB = ${FILE_COUNT} files x $((RSS_BASE_PER_FILE/1024)) KiB + ${FANOUT_EDGES} fan-out edges x ${RSS_PER_FANOUT_EDGE} B + ${FANOUT_CANDIDATES} candidates x ${RSS_PER_CANDIDATE} B"
# Budget re-derived 2026-08-16 when the grammar matrix went from 5 linked
# languages to 32: the port now parses 22 more grammars and 1,110 files instead
# of 1,088, so the 5 s budget measured work the build no longer does. Quiet-run
# measurements after the change: 3.0 s, 3.65 s, 4.0 s. The ledger added by D17
# was A/B-measured and is not a factor (4,007 ms without it vs 3,650 ms with —
# inside the noise). 10 s keeps roughly 2.5x headroom over the measured cost so
# the gate catches a real regression rather than the machine being busy, which
# is what a wall-clock budget on a shared runner actually observes.
[ "$MS" -lt 10000 ] || { echo "GATE FAIL: build >= 10 s"; exit 1; }
[ "$BYTES" -lt "$GATE" ] || { echo "GATE FAIL: db >= $((GATE/1024/1024)) MiB for ${FILE_COUNT} files"; exit 1; }
[ "$RSS" -lt "$RSS_GATE" ] || { echo "GATE FAIL: peak RSS $((RSS/1024/1024)) MiB >= $((RSS_GATE/1024/1024)) MiB"; exit 1; }
[ "$RSS" -lt "$RSS_BUDGET" ] || {
  echo "GATE FAIL: peak RSS $((RSS/1024/1024)) MiB >= scale-invariant budget $((RSS_BUDGET/1024/1024)) MiB for ${FILE_COUNT} files, ${FANOUT_EDGES} fan-out edges and ${FANOUT_CANDIDATES} candidates"; exit 1; }

step "6/9 memory-model probe — bound the cost per unit of ambiguity fan-out"
# The two gates above bound this repository. Neither says anything about the
# 12,831-file corpus the port is actually run against: an absolute ceiling
# calibrated on 1.1k files is silent there, and one raised to cover it would
# stop catching a regression here. A coefficient is the same number at any
# corpus size, so this probe builds a synthetic corpus whose fan-out is known by
# construction, subtracts a paired zero-ambiguity control to measure the base
# term rather than model it, and bounds bytes-per-*candidate* — the quantity the
# memory actually tracks since `AMBIGUOUS_FANOUT_CAP` decoupled it from emitted
# edges — plus the model that combines the two terms, plus how the per-candidate
# cost *moves* when the candidate list doubles, which is the direct test of the
# shared-`Arc` invariant. It prints exactly what it capped.
./tools/memory_model_probe.sh || { echo "GATE FAIL: memory-model probe"; exit 1; }

step "7/9 growth gate — repeated builds must plateau"
# SC1/SC7: generations and the extraction cache were both retained forever, so
# the database grew by O(repository size) on every build. Nothing here caught
# it because every gate built exactly once into a fresh database. This builds
# repeatedly into one store and requires the size to stop growing.
GROWTH_DB="$TMP2/growth.sqlite"
GROWTH_SRC="$TMP2/growth-src"
mkdir -p "$GROWTH_SRC"
cp -R testdata/. "$GROWTH_SRC/" 2>/dev/null || true
SIZES=""
for round in 1 2 3 4 5; do
  printf '\n# growth probe %s\n' "$round" >> "$GROWTH_SRC/churn.py"
  "$DEVMAP_BIN" --db "$GROWTH_DB" --progress never build "$GROWTH_SRC" >/dev/null
  SIZES="$SIZES $(stat -f%z "$GROWTH_DB" 2>/dev/null || stat -c%s "$GROWTH_DB")"
done
set -- $SIZES
S3=$3; S5=$5
GENS=$(sqlite3 "$GROWTH_DB" "SELECT COUNT(*) FROM generations")
# SC27: the plateaued size is the one a running repository lives at, and until
# now nothing bounded it — only its *stability* was checked. A regression that
# doubled per-generation storage would plateau just as flatly, one gate short of
# being caught.
GROWTH_FILES=$(sqlite3 "$GROWTH_DB" "SELECT COUNT(*) FROM generation_files WHERE generation_id = (SELECT max(id) FROM generations)")
RETAINED=$(grep -oE 'DB_SIZE_GATE_RETAINED_GENERATIONS: u64 = [0-9]+' \
  devmap-extract/src/model.rs | grep -oE '[0-9]+$')
[ -n "$RETAINED" ] || { echo "GATE FAIL: cannot read DB_SIZE_GATE_RETAINED_GENERATIONS"; exit 1; }
STEADY_GATE=$(( 60*1024*1024 ))
STEADY_SCALED=$(( GROWTH_FILES * PER_FILE_KIB * 1024 * RETAINED ))
if [ "$STEADY_SCALED" -gt "$STEADY_GATE" ]; then STEADY_GATE=$STEADY_SCALED; fi
echo "growth bytes:$SIZES generations retained: $GENS (steady gate $((STEADY_GATE/1024/1024)) MiB for $GROWTH_FILES files)"
[ "$S5" -lt "$STEADY_GATE" ] || {
  echo "GATE FAIL: steady-state store $S5 bytes >= $STEADY_GATE for $GROWTH_FILES files"; exit 1; }
[ "$GENS" -le 2 ] || { echo "GATE FAIL: $GENS generations retained after 5 builds; retention is unbounded"; exit 1; }
# Builds 3->5 must not grow the store. Some slack for page-level churn only.
LIMIT=$(( S3 + S3 / 20 + 65536 ))
[ "$S5" -le "$LIMIT" ] || {
  echo "GATE FAIL: store grew from $S3 to $S5 bytes between builds 3 and 5; growth is unbounded"; exit 1; }

step "8/9 incremental-vs-cold equivalence — a rebuild must not drift"
# SC16: the determinism gate compares two *cold* builds and the growth gate
# compares size, so neither could see an incremental build diverging from a
# cold one. This runs the real thing: repeated rebuild cycles over one store,
# failing on graph drift or unbounded growth.
SOAK_DIR=$(mktemp -d)
rsync -a --chmod=u+w ./testdata/ "$SOAK_DIR/" >/dev/null 2>&1
# Five cycles is a smoke run: the soak asserts the digest returns after every
# restore and says in its verdict that growth was not measured. The plateau
# needs 40 cycles or more and is a separate, longer run.
if ./tools/soak.sh "$SOAK_DIR" 5 2>&1 | tail -2 | grep -q "SOAK SMOKE OK"; then
  echo "incremental equivalence OK (5 cycles, growth not asserted)"
else
  echo "GATE FAIL: incremental build drifted from cold"; rm -rf "$SOAK_DIR"; exit 1
fi
rm -rf "$SOAK_DIR"

step "9/9 mutation testing (optional)"
if [ "$MUTANTS" -eq 1 ]; then
  command -v cargo-mutants >/dev/null || {
    echo "cargo-mutants is not installed; install it explicitly after dependency approval" >&2
    exit 2
  }
  cargo mutants -p devmap-resolve -p devmap-analyze --timeout 60 || {
    echo "Missed mutants above = tests that cannot fail. See INTEGRITY.md §Vacuous tests."; exit 1; }
else
  echo "skipped (pass --mutants to run)"
fi

printf '\n\033[1mALL GATES GREEN\033[0m\n'
