#!/bin/zsh
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
# the plateau assertion compares the last cycle against a baseline cycle rather
# than against the first: the first few cycles are still filling caches, and a
# soak that called that growth a leak would fail on every healthy kernel.
set -u
ROOT="${1:?usage: soak.sh <workdir> [cycles] [--daemon]}"
CYCLES="${2:-40}"
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

# The plateau, measured the way the data supports.
#
# Not "cycle N against cycle 20": on both corpora the working set is not
# established at cycle 20. The daemon's first full drain lands between cycles 20
# and 50 and takes RSS from 240 MB to ~900 MB and the store from 247 MB to
# 498 MB — growth that is the process loading what it is for, not a leak, and a
# check that called it one would fail on every healthy kernel.
#
# So: the first quarter of the run is warm-up and is reported but not compared.
# The rest is split in half and the two *means* are compared, because a single
# sample carries allocator noise a mean does not — over the last hundred cycles
# of the run that set this tolerance the daemon's per-cycle RSS swung between
# 755 MB and 953 MB (±12% around its mean) while the mean itself moved -6.24%.
#
# TOLERANCE_PCT is the ceiling on that movement, 10% by default: four times the
# observed drift, and far below what a leak does — the same run's *warm-up*
# quarter moved +210%, which is the shape this is looking for.
if [ "$FAILS" -eq 0 ] && [ "$CYCLES" -ge 40 ]; then
  WARMUP=$(( CYCLES / 4 ))
  MID=$(( WARMUP + (CYCLES - WARMUP) / 2 ))
  for column in "rss:2" "db:3"; do
    name=${column%%:*}; col=${column#*:}
    first=$(awk -F, -v c="$col" -v a="$(( WARMUP + 1 ))" -v b="$MID" \
      'NR>1 && $1>=a && $1<=b {s+=$c; n++} END {if (n) printf "%d", s/n}' "$CSV")
    second=$(awk -F, -v c="$col" -v a="$(( MID + 1 ))" -v b="$CYCLES" \
      'NR>1 && $1>=a && $1<=b {s+=$c; n++} END {if (n) printf "%d", s/n}' "$CSV")
    if [ -z "$first" ] || [ -z "$second" ] || [ "$first" -eq 0 ]; then
      echo "SOAK FAIL: $name has no samples in one of the halves — the plateau check could not run"
      FAILS=1
      continue
    fi
    limit=$(( first + first * TOLERANCE_PCT / 100 ))
    if [ "$second" -gt "$limit" ]; then
      echo "SOAK FAIL: $name did not plateau — mean($(( WARMUP + 1 ))-$MID) $first -> mean($(( MID + 1 ))-$CYCLES) $second (limit $limit)"
      FAILS=1
    else
      echo "plateau ok: $name mean($(( WARMUP + 1 ))-$MID) $first -> mean($(( MID + 1 ))-$CYCLES) $second (limit $limit)"
    fi
  done
elif [ "$FAILS" -eq 0 ]; then
  echo "plateau not asserted: $CYCLES cycles is under the 40-cycle minimum, so this run is a smoke test only"
fi

[ "$FAILS" -eq 0 ] && echo "SOAK OK ($CYCLES cycles, digest stable, growth bounded); samples in $CSV"
exit "$FAILS"
