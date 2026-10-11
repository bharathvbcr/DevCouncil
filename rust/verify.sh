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

# Gates that were not run, printed again at the end: "green" must not read
# the same whether a gate ran or was skipped.
NOT_RUN=()
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

step "1/8 format"
cargo fmt --all -- --check

step "2/8 clippy (-D warnings)"
cargo clippy --workspace --all-targets -- -D warnings

# The host binary gate 3 needs, built the way the workflows build it.
#
# `dc-verify`'s json_contract drives `devcouncil --json` as a real process —
# two real file descriptors are the only way to tell a banner on stdout from
# one on stderr — and since 2026-09-14 it fails rather than skips when the
# binary is absent, because a gate that ran nothing must not report what a gate
# that ran and passed reports. That makes the host a fixture of `cargo test
# --workspace`, so this pipeline builds it, exactly as `analysis-plane.yml` and
# `rust.yml` do.
#
# The host is not a dependency-free Go build. go.mod replaces
# github.com/bharathvbcr/gusset with the sibling checkout at <repo>/../gusset,
# and cgo links rust/gusset-engine/target/release/libgusset.a. That archive is
# gitignored. CI checks the sibling out in .github/actions/setup-gusset and
# builds the archive there; locally this step builds the archive when the
# sibling is already on disk. A missing sibling fails here, by name, rather
# than as "replacement directory does not exist" after Go has started.
step "3/8 tests — host fixture"
if [ -n "${DEVCOUNCIL_BIN:-}" ]; then
  echo "DEVCOUNCIL_BIN is set ($DEVCOUNCIL_BIN); not building the host"
elif ! command -v go >/dev/null 2>&1; then
  echo "GATE FAIL: no \`go\` on PATH, and gate 3 needs the DevCouncil host binary —" >&2
  echo "dc-verify's json_contract runs it as a real process and fails when it is absent." >&2
  echo "Install Go, or point DEVCOUNCIL_BIN at a host built from this tree." >&2
  exit 1
else
  repo_root="$(cd .. && pwd)"
  gusset_dir="$(dirname "$repo_root")/gusset"
  archive="gusset-engine/target/release/libgusset.a"
  if [[ ! -f "$gusset_dir/go.mod" ]]; then
    echo "GATE FAIL: the Go host replaces github.com/bharathvbcr/gusset with $gusset_dir, which is not a checkout." >&2
    echo "Check out https://github.com/bharathvbcr/gusset there. CI does this in .github/actions/setup-gusset." >&2
    exit 1
  fi
  # The sibling must be the revision CI builds; a drifted sibling is a failure,
  # not a warning (see gusset-engine/check-pin.sh).
  gusset-engine/check-pin.sh || {
    echo "GATE FAIL: the Gusset sibling at $gusset_dir is not at the setup-gusset pin." >&2
    exit 1
  }
  # Always the incremental build, never "only when missing": that linked
  # whatever archive an older checkout left behind. cgo-env.sh also keys Go's
  # caches on the archive's hash, without which a rebuilt archive is not
  # relinked and a cached test result is replayed against the old one.
  echo "building gusset-engine release staticlib (libgusset.a is not in git)"
  # Captured before eval: `eval "$(failing)" || …` evaluates the empty output,
  # succeeds, and the build then links whatever archive is lying around.
  engine_exports="$(gusset-engine/cgo-env.sh --export)" || {
    echo "GATE FAIL: gusset-engine/cgo-env.sh could not build $archive" >&2
    exit 1
  }
  eval "$engine_exports"
  go -C ../backend/go_orchestrator build -o bin/ ./cmd/devcouncil
  go -C ../backend/go_orchestrator test -count=1 ./gussetfn
  # The whole policy suite again with the engine as every gate's default
  # matcher, plus a decision-for-decision comparison against fnmatch.
  go -C ../backend/go_orchestrator test -count=1 -tags gussetengine ./policy
  ../backend/go_orchestrator/bin/devcouncil gusset-check
fi

step "3/8 tests"
cargo test --workspace

if [ "$QUICK" -eq 1 ]; then
  echo "NOT RUN (quick mode): determinism, memory ceiling, growth plateau, incremental equivalence, mutation testing"
  printf '\n\033[1mQUICK GATES GREEN; the full gate suite was NOT RUN\033[0m\n'
  exit 0
fi

step "release worker panic recovery (executable, not test profile)"
cargo run --release --locked -q -p devmap-serve --example worker_recovery_probe

# Where `cargo` actually put the release binary. Steps 5 and 7 run it directly
# rather than through `cargo run`, so they have to agree with cargo about the
# path — and `CARGO_TARGET_DIR` is set by anyone running two lanes against one
# checkout, which is a layout this repository's own guidance recommends. Before
# this, such a run reported "could not measure peak RSS", which is true and is
# the wrong reason: the measurement was fine and the binary was not there.
DEVMAP_BIN="${CARGO_TARGET_DIR:-$(pwd)/target}/release/devmap"
# Exported so the soak (`tools/soak`) measures the same
# binary these gates did, rather than each resolving it again.
export DEVMAP_BIN

step "4/8 determinism — two clean builds must produce identical graph digests"
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

step "5/8 self-build gates — DevCouncil repo, release"
# Peak RSS is measured, not assumed, and an unavailable reading fails this step
# rather than skipping it. The measurement itself lives in tools/peak_rss.sh so
# this gate and the soak share one implementation.
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
# Recalibrated 2026-09-11 after AmbiguousGlobal sites stopped emitting capped
# edge fan-out and instead record one unresolved ledger row with the full
# Cold self-build peak is bounded per file: 896 KiB/file was ~1.22x the rate
# measured when this gate was derived (610 MiB over 851 files), enough headroom
# for allocator noise and not enough to hide a return to multi-GiB SC3
# behaviour. That figure was measured while an ambiguous site still emitted up
# to sixteen edges and carried a second, per-candidate term for the shared
# candidate list. S2 made ambiguity ledger-only: the site emits no edge and its
# one ledger row carries the candidate list under `ambiguity_evidence_bytes`
# (pinned by the `ambiguity_budget` tests), so both terms are gone and the
# per-file base is the whole budget. Re-measure the release self-build and
# tighten the base if the rate has fallen.
RSS_BASE_PER_FILE=$(( 896 * 1024 ))
RSS_BUDGET=$(( FILE_COUNT * RSS_BASE_PER_FILE ))

# The fan-out metric stays, as a guard that the mechanism stays gone: derived
# from the persisted store, and failing closed if it cannot be derived.
FANOUT=$(./tools/fanout.sh "$TMP1/self.sqlite") || {
  echo "GATE FAIL: could not derive the ambiguity fan-out metrics"; exit 1; }
FANOUT_EDGES=$(printf '%s\n' "$FANOUT" | tr ' ' '\n' | awk -F= '$1=="fanout_edges" {print $2; exit}')
[[ "$FANOUT_EDGES" =~ ^[0-9]+$ ]] || { echo "GATE FAIL: malformed fan-out metrics: $FANOUT"; exit 1; }
echo "$FANOUT"
[ "$FANOUT_EDGES" -eq 0 ] || { echo "GATE FAIL: ${FANOUT_EDGES} ambiguous fan-out edges; ambiguity is ledger-only"; exit 1; }

echo "build ${MS} ms, db $((BYTES/1024/1024)) MiB (cold), files ${FILE_COUNT}, gate $((GATE/1024/1024)) MiB, peak RSS $((RSS/1024/1024)) MiB"
echo "rss budget $((RSS_BUDGET/1024/1024)) MiB = ${FILE_COUNT} files x $((RSS_BASE_PER_FILE/1024)) KiB"
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
  echo "GATE FAIL: peak RSS $((RSS/1024/1024)) MiB >= scale-invariant budget $((RSS_BUDGET/1024/1024)) MiB for ${FILE_COUNT} files"; exit 1; }

step "6/8 growth gate — repeated builds must plateau"
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

step "7/8 incremental-vs-cold equivalence — a rebuild must not drift"
# SC16: the determinism gate compares two *cold* builds and the growth gate
# compares size, so neither could see an incremental build diverging from a
# cold one. This runs the real thing: repeated rebuild cycles over one store,
# failing on graph drift or unbounded growth.
SOAK_DIR=$(mktemp -d)
rsync -a --chmod=u+w ./testdata/ "$SOAK_DIR/" >/dev/null 2>&1
# Five cycles is a smoke run: the soak asserts the digest returns after every
# restore and says in its verdict that growth was not measured. The plateau
# needs 40 cycles or more and is a separate, longer run.
# Built first, so a soak that does not compile is not reported as drift. Both
# the exit status and the verdict line must agree; on failure the soak's own
# last lines say which check it was.
cargo build --release --locked -q -p devmap-soak || {
  echo "GATE FAIL: the soak harness (devmap-soak) did not build"; rm -rf "$SOAK_DIR"; exit 1; }
SOAK_STATUS=0
SOAK_OUT=$(cargo run --release --locked -q -p devmap-soak -- "$SOAK_DIR" 5 2>&1) || SOAK_STATUS=$?
if [ "$SOAK_STATUS" -eq 0 ] && printf '%s\n' "$SOAK_OUT" | tail -2 | grep -q "SOAK SMOKE OK"; then
  echo "incremental equivalence OK (5 cycles, growth not asserted)"
else
  printf '%s\n' "$SOAK_OUT" | tail -8
  echo "GATE FAIL: the incremental-equivalence soak failed (exit $SOAK_STATUS)"; rm -rf "$SOAK_DIR"; exit 1
fi
rm -rf "$SOAK_DIR"

step "8/8 mutation testing (optional)"
if [ "$MUTANTS" -eq 1 ]; then
  command -v cargo-mutants >/dev/null || {
    echo "cargo-mutants is not installed; install it explicitly after dependency approval" >&2
    exit 2
  }
  cargo mutants -p devmap-resolve -p devmap-analyze --timeout 60 || {
    echo "Missed mutants above = tests that cannot fail. See INTEGRITY.md §Vacuous tests."; exit 1; }
else
  echo "NOT RUN: mutation testing (pass --mutants to run it; .github/workflows/mutants.yml runs it weekly)"
  NOT_RUN+=("mutation testing")
fi

if [ "${#NOT_RUN[@]}" -gt 0 ]; then
  printf '\n\033[1mGATES GREEN, but NOT RUN: %s\033[0m\n' "${NOT_RUN[*]}"
else
  printf '\n\033[1mALL GATES GREEN\033[0m\n'
fi
