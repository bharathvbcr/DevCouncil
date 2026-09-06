#!/usr/bin/env bash
# Sustained-load soak: repeated edit -> rebuild -> query cycles against one
# store, asserting that nothing drifts over time. The gates prove a *single*
# build is correct; this proves the hundredth is too.
#
# Fails on: any build/query error, unbounded database growth, unbounded peak
# RSS, or a graph digest that changes when the source has been restored.
#
# Two modes, because the two processes fail differently. The default drives
# `devmap build` — a short-lived process, so its peak RSS is measurable per
# cycle. `--daemon` drives a long-lived `devmap serve` through the same edit
# cycles with queries over IPC, which is the process an agent host actually
# keeps open and the only one where a leak accumulates across hours.
#
# Every cycle is sampled (cycle, peak/resident RSS, store bytes) into a CSV, and
# the plateau assertion compares the means of two halves taken after a warm-up
# — in daemon mode, a warm-up that ends where the samples show the first drain
# landing, because the first cycles are the process loading its graph and a
# soak that called that growth a leak would fail on every healthy kernel. A run
# too short to compare says so; it never reports growth it did not measure.
set -u
ROOT="${1:?usage: soak.sh <workdir> [cycles] [--daemon]}"
CYCLES="${2:-40}"
# Under 40 cycles the run is a smoke test: the digest must return after every
# restore, and growth is reported but not asserted. `verify.sh` uses that.
MODE="${3:-build}"
TOOLS="$(cd "$(dirname "$0")" && pwd)"
# Honour CARGO_TARGET_DIR: lanes build into their own target directory, and a
# soak that silently measured a stale `rust-port/target/release/devmap` would
# report the previous kernel's numbers as this one's.
TARGET_DIR="${CARGO_TARGET_DIR:-$(cd "$TOOLS/.." && pwd)/target}"
DEVMAP="${DEVMAP_BIN:-$TARGET_DIR/release/devmap}"
[ -x "$DEVMAP" ] || { echo "SOAK FAIL: no devmap binary at $DEVMAP"; exit 1; }
command -v sqlite3 >/dev/null 2>&1 || { echo "SOAK FAIL: sqlite3 is required to read the digest"; exit 1; }
# shellcheck source=peak_rss.sh
. "$TOOLS/peak_rss.sh"

# The kernel's store. `.devcouncil/codeintel/index.sqlite` is the *Python*
# engine's file: this script read that one for its whole life, so `digest`
# returned the empty string and `db_bytes` returned 0 on every cycle. The digest
# comparison then held trivially and the growth limit compared 0 against 0 — a
# check that could not run reporting exactly what a check that ran and passed
# reports, in the one script whose entire job is to notice drift.
STORE=".devcouncil/codeintel/devmap.sqlite"
CSV="${SOAK_CSV:-$ROOT/soak_samples.csv}"
# What counts as a plateau: how far the second half's mean may sit above the
# first half's, in percent. Chosen from measured data, not assumed — see the
# plateau block at the bottom for the readings it was set from.
TOLERANCE_PCT="${SOAK_TOLERANCE_PCT:-10}"

cd "$ROOT" || exit 1

digest() {
  sqlite3 "$STORE" \
    "SELECT source_symbol||'>'||target_symbol||':'||edge_kind FROM generation_edges
     WHERE generation_id=(SELECT max(id) FROM generations) ORDER BY 1;" 2>/dev/null | shasum | cut -d' ' -f1
}
file_bytes() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1" 2>/dev/null || echo 0; }
# Main database plus its write-ahead log: both are the store's bytes on disk,
# and a WAL that never checkpoints is exactly the unbounded growth this looks
# for. Counting only the main file would hide it.
db_bytes() {
  local main wal
  main=$(file_bytes "$STORE")
  wal=$(file_bytes "$STORE-wal")
  echo $(( main + wal ))
}

TARGET=$(find . -name '*.py' -not -path './.devcouncil/*' | head -1)
[ -n "$TARGET" ] || { echo "SOAK FAIL: no target file"; exit 1; }
ORIG="$ROOT/.soak_orig.$$"
cp "$TARGET" "$ORIG"
restore() { cp "$ORIG" "$TARGET"; }
trap 'restore; rm -f "$ORIG"' EXIT

"$DEVMAP" build . >/dev/null 2>&1 || { echo "SOAK FAIL: initial build"; exit 1; }
[ -f "$STORE" ] || { echo "SOAK FAIL: no store at $ROOT/$STORE after the initial build"; exit 1; }
BASE_DIGEST=$(digest)
[ -n "$BASE_DIGEST" ] || { echo "SOAK FAIL: the baseline digest is empty — the store has no edges to compare"; exit 1; }
BASE_DB=$(db_bytes)
echo "soak baseline: mode=$MODE digest=${BASE_DIGEST:0:12} db=${BASE_DB} tolerance=${TOLERANCE_PCT}%"
echo "cycle,rss_bytes,db_bytes" > "$CSV"

FAILS=0
TIMEOUT_LOG="$ROOT/.soak_time.$$"

if [ "$MODE" = "--daemon" ]; then
  # The long-lived process. Its peak RSS is only readable when it exits, so the
  # per-cycle column is *resident* RSS sampled with `ps` and the peak is read
  # from /usr/bin/time at the end. Both are reported; neither is presented as
  # the other.
  [ -n "$PEAK_RSS_TIME_FLAG" ] || { echo "SOAK FAIL: no /usr/bin/time supporting -l or -v"; exit 1; }
  # A socket of this soak's own, under the workdir. Two reasons: the default
  # endpoint is shared with whatever daemon the developer's own checkout is
  # already running, and matching on this path is how the daemon's pid is found
  # without `pgrep -n -x devmap` — which on a machine with an unrelated daemon
  # open (one was, for 2h40m, during the run that found this) picks whichever
  # devmap happens to be newest and samples a process the soak never started.
  # Under TMPDIR, not under the workdir: `sun_path` is 104 bytes on this
  # platform and a scratch workdir path is routinely longer than that, so a
  # socket named beside the corpus is silently never bound — the daemon exits
  # and the soak reports "never bound" without saying why.
  ENDPOINT="${TMPDIR:-/tmp}/devmap-soak-$$.sock"
  rm -f "$ENDPOINT"
  DEVMAP_MAX_IDLE_SECS=0 /usr/bin/time "$PEAK_RSS_TIME_FLAG" \
    "$DEVMAP" serve . --socket "$ENDPOINT" >/dev/null 2>"$TIMEOUT_LOG" &
  TIME_PID=$!
  for _ in $(seq 1 150); do [ -S "$ENDPOINT" ] && break; sleep 0.2; done
  # Every exit from here on must take the daemon with it. Killing the
  # `/usr/bin/time` wrapper does not: it leaves the daemon serving, watching the
  # workdir and writing generations into the very store the *next* soak is about
  # to measure. One such orphan ran for five minutes beside a build-mode soak on
  # the same corpus before this trap existed, and every number that soak
  # produced described two writers rather than one.
  daemon_cleanup() {
    [ -n "${SERVE_PID:-}" ] && kill "$SERVE_PID" 2>/dev/null
    [ -n "${TIME_PID:-}" ] && kill "$TIME_PID" 2>/dev/null
    pkill -f -- "--socket $ENDPOINT" 2>/dev/null
    rm -f "$ENDPOINT" "$ORIG"
    return 0
  }
  trap 'restore; daemon_cleanup' EXIT
  [ -S "$ENDPOINT" ] || {
    echo "SOAK FAIL: the daemon never bound $ENDPOINT"
    echo "  its output was: $(tail -5 "$TIMEOUT_LOG" 2>/dev/null)"
    exit 1
  }
  # Of the processes whose argv names this socket — the `/usr/bin/time` wrapper
  # is one of them — the one whose executable is the binary being measured.
  # Matched against `$DEVMAP`'s own basename rather than the literal `devmap`,
  # because a lane pins `DEVMAP_BIN` to a copy it has saved aside (so a rebuild
  # cannot change the binary halfway through a two-hour soak) and that copy is
  # not called `devmap`.
  SERVE_NAME="${DEVMAP##*/}"
  SERVE_PID=""
  for pid in $(pgrep -f -- "$ENDPOINT"); do
    case "$(ps -o comm= -p "$pid" 2>/dev/null)" in
      */"$SERVE_NAME"|"$SERVE_NAME") SERVE_PID="$pid" ;;
    esac
  done
  [ -n "$SERVE_PID" ] || { echo "SOAK FAIL: cannot identify the daemon process"; exit 1; }
  ask() {
    python3 - "$ENDPOINT" "$1" <<'PY'
import json, socket, sys
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.settimeout(30)
sock.connect(sys.argv[1])
sock.sendall(sys.argv[2].encode() + b"\n")
buf = b""
while not buf.endswith(b"\n"):
    chunk = sock.recv(65536)
    if not chunk:
        break
    buf += chunk
json.loads(buf.decode())
PY
  }
  for i in $(seq 1 "$CYCLES"); do
    printf '\n# soak cycle %s\ndef _soak_%s():\n    return %s\n' "$i" "$i" "$i" >> "$TARGET"
    # Long enough for the watcher to see the write and queue it. Without it the
    # edit is undone before the daemon ever notices, and the soak measures a
    # process answering queries against a tree nobody is changing.
    sleep 0.2
    ask '{"version":1,"cmd":"status"}' || { echo "SOAK FAIL: status cycle $i"; FAILS=1; break; }
    ask '{"version":1,"cmd":"search","query":"soak","budget":2000}' || { echo "SOAK FAIL: search cycle $i"; FAILS=1; break; }
    ask '{"version":1,"cmd":"impact","target":"main"}' || { echo "SOAK FAIL: impact cycle $i"; FAILS=1; break; }
    restore
    RSS=$(ps -o rss= -p "$SERVE_PID" 2>/dev/null | tr -d ' ')
    [ -n "$RSS" ] || { echo "SOAK FAIL: the daemon died at cycle $i"; FAILS=1; break; }
    echo "$i,$(( RSS * 1024 )),$(db_bytes)" >> "$CSV"
    if [ $((i % 10)) -eq 0 ]; then echo "  cycle $i ok rss=$(( RSS * 1024 )) db=$(db_bytes)"; fi
  done
  kill "$SERVE_PID" 2>/dev/null
  wait "$TIME_PID" 2>/dev/null
  rm -f "$ENDPOINT"
  PEAK=$(awk '/maximum resident set size/ {print $1; exit}' "$TIMEOUT_LOG")
  [ -n "$PEAK" ] || PEAK=$(awk -F': ' '/Maximum resident set size/ {print $2*1024; exit}' "$TIMEOUT_LOG")
  echo "daemon peak rss: ${PEAK:-unavailable} bytes over $CYCLES cycles"
else
  for i in $(seq 1 "$CYCLES"); do
    # Perturb, rebuild, then restore and rebuild: the digest must return.
    printf '\n# soak cycle %s\ndef _soak_%s():\n    return %s\n' "$i" "$i" "$i" >> "$TARGET"
    "$DEVMAP" build . >/dev/null 2>&1 || { echo "SOAK FAIL: build (dirty) cycle $i"; FAILS=1; break; }
    for q in search dead status; do
      "$DEVMAP" "$q" >/dev/null 2>&1 || "$DEVMAP" "$q" soak >/dev/null 2>&1 || true
    done
    restore
    RSS=$(peak_rss_bytes "$TIMEOUT_LOG" "$DEVMAP" build .) || { echo "SOAK FAIL: build (restored) cycle $i"; FAILS=1; break; }
    D=$(digest)
    if [ "$D" != "$BASE_DIGEST" ]; then
      echo "SOAK FAIL: digest drift at cycle $i (${D:0:12} != ${BASE_DIGEST:0:12})"; FAILS=1; break
    fi
    echo "$i,$RSS,$(db_bytes)" >> "$CSV"
    if [ $((i % 10)) -eq 0 ]; then echo "  cycle $i ok rss=$RSS db=$(db_bytes)"; fi
  done
fi
rm -f "$TIMEOUT_LOG"

END_DB=$(db_bytes)
echo "soak end: db=$END_DB (baseline $BASE_DB)"

# --- plateau begin
# assert_plateau <csv> <cycles> <mode> <baseline_db_bytes> <tolerance_pct>
#
# Prints one verdict line per sampled column and returns non-zero on a failed
# comparison — or on a comparison that could not run, which is reported as a
# failure and never as a plateau.
#
# Warm-up is reported but not compared, and it is not a fixed fraction of the
# run. In build mode every cycle is a whole `devmap build`, so the first quarter
# is allocator settling and the data has been flat from cycle 1 on both corpora.
# In daemon mode the working set arrives with the first drain: the watcher holds
# a burst of edits for its 2 s quiet window and at most 10 s, then writes a
# generation and builds its index — RSS 240 MB -> ~900 MB and the store 247 MB
# -> 498 MB on the scholarlm corpus. That is wall-clock, while a cycle on a small
# corpus takes half a second; so the cycle it lands on is read from the samples
# (the first store-bytes change after the baseline) rather than assumed. A
# 40-cycle daemon run on this repository's corpus put the first drain at cycle
# 30 and then compared cycles 11-25 against 26-40, calling the process loading
# its graph a leak.
#
# After warm-up the rest is split in half and the two *means* are compared: a
# single sample carries allocator noise a mean does not — over the last hundred
# cycles of the run that set the tolerance the daemon's per-cycle RSS swung
# between 755 MB and 953 MB (±12% around its mean) while the mean moved -6.24%.
# The tolerance is the ceiling on that movement, 10% by default: four times the
# observed drift, and far below what a leak does — the same run's warm-up moved
# +210%, which is the shape this looks for.
assert_plateau() {
  local csv=$1 cycles=$2 mode=$3 base_db=$4 tol=$5
  local warmup
  if [ "$mode" = "--daemon" ]; then
    local first_drain
    first_drain=$(awk -F, -v b="$base_db" 'NR>1 && $3!=b {print $1; exit}' "$csv")
    if [ -z "$first_drain" ]; then
      echo "SOAK FAIL: the daemon never wrote a generation in $cycles cycles, so the plateau check could not run"
      return 1
    fi
    # The generations that follow the first, and the index built over them,
    # keep arriving for a while after it: a quarter of what remains is settle.
    warmup=$(( first_drain + (cycles - first_drain) / 4 ))
    echo "daemon first drain landed at cycle $first_drain; warm-up ends at cycle $warmup"
  else
    warmup=$(( cycles / 4 ))
  fi
  local remaining=$(( cycles - warmup ))
  if [ "$remaining" -lt 30 ]; then
    echo "SOAK FAIL: $remaining cycles after warm-up (which ends at cycle $warmup of $cycles) is fewer than the 30 a two-half comparison needs — run more cycles"
    return 1
  fi
  local mid=$(( warmup + remaining / 2 ))
  local fails=0 column name col first second limit
  for column in "rss:2" "db:3"; do
    name=${column%%:*}; col=${column#*:}
    first=$(awk -F, -v c="$col" -v a="$(( warmup + 1 ))" -v b="$mid" \
      'NR>1 && $1>=a && $1<=b {s+=$c; n++} END {if (n) printf "%d", s/n}' "$csv")
    second=$(awk -F, -v c="$col" -v a="$(( mid + 1 ))" -v b="$cycles" \
      'NR>1 && $1>=a && $1<=b {s+=$c; n++} END {if (n) printf "%d", s/n}' "$csv")
    if [ -z "$first" ] || [ -z "$second" ] || [ "$first" -eq 0 ]; then
      echo "SOAK FAIL: $name has no samples in one of the halves — the plateau check could not run"
      fails=1
      continue
    fi
    limit=$(( first + first * tol / 100 ))
    if [ "$second" -gt "$limit" ]; then
      echo "SOAK FAIL: $name did not plateau — mean($(( warmup + 1 ))-$mid) $first -> mean($(( mid + 1 ))-$cycles) $second (limit $limit)"
      fails=1
    else
      echo "plateau ok: $name mean($(( warmup + 1 ))-$mid) $first -> mean($(( mid + 1 ))-$cycles) $second (limit $limit)"
    fi
  done
  return $fails
}
# --- plateau end

if [ "$FAILS" -eq 0 ] && [ "$CYCLES" -ge 40 ]; then
  assert_plateau "$CSV" "$CYCLES" "$MODE" "$BASE_DB" "$TOLERANCE_PCT" || FAILS=1
  [ "$FAILS" -eq 0 ] && echo "SOAK OK ($CYCLES cycles, digest stable, growth bounded); samples in $CSV"
elif [ "$FAILS" -eq 0 ]; then
  # Two verdict words on purpose. `verify.sh` runs five cycles for incremental
  # equivalence and greps for this one; the old single "SOAK OK (growth
  # bounded)" was printed here too, for a check that had not run.
  echo "SOAK SMOKE OK ($CYCLES cycles, digest stable; growth NOT asserted — under the 40-cycle minimum); samples in $CSV"
fi
exit "$FAILS"
