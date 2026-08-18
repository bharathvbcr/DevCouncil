# shellcheck shell=bash
# Peak resident-set measurement, shared by `verify.sh` and the memory-model
# probe so the two cannot drift apart. Source it; do not execute it.
#
# Peak RSS is measured, not assumed. A 10.45 GiB peak on a 4.7k-file repository
# (SC3) survived every green run of the gate suite because nothing measured
# memory: the gates covered wall time and database size only. A check that
# cannot run must never report what a check that ran and passed reports, so an
# unavailable reading is a failure here, never a skip.
#
# `/usr/bin/time` differs between the two platforms the gates run on: BSD/macOS
# spells the verbose flag `-l` and reports bytes, GNU spells it `-v` and reports
# kibibytes. The flag is probed once against `true` rather than by retrying the
# real command, because retrying would run the measured build twice.

PEAK_RSS_TIME_FLAG=""
if command -v /usr/bin/time >/dev/null 2>&1; then
  if /usr/bin/time -l true >/dev/null 2>&1; then
    PEAK_RSS_TIME_FLAG="-l"
  elif /usr/bin/time -v true >/dev/null 2>&1; then
    PEAK_RSS_TIME_FLAG="-v"
  fi
fi

# usage: peak_rss_bytes <outfile> <command...>  -> prints peak RSS in bytes
peak_rss_bytes() {
  local out="$1"; shift
  [ -n "$PEAK_RSS_TIME_FLAG" ] || {
    echo "no /usr/bin/time supporting -l (BSD) or -v (GNU) is available" >&2
    return 1
  }
  /usr/bin/time "$PEAK_RSS_TIME_FLAG" "$@" 2>"$out" >/dev/null || return 1
  local v
  v=$(awk '/maximum resident set size/ {print $1; exit}' "$out")                  # macOS: bytes
  if [ -z "$v" ]; then
    v=$(awk -F': ' '/Maximum resident set size/ {print $2*1024; exit}' "$out")    # GNU: KiB
  fi
  [ -n "$v" ] || return 1
  printf '%s\n' "$v"
}
