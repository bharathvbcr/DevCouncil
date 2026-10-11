#!/usr/bin/env bash
# Measures what the integration-test targets of some crates cost to build:
# wall and CPU seconds, the number of test executables, and their size.
#
#   tools/test_link_ab.sh <label> <runs> <crate>...
#
# Each run first `cargo clean -p`s the crates, so third-party dependencies stay
# built and the measurement is the crate, its test targets and their links.
# Use a private CARGO_TARGET_DIR. For an A/B, run the same command on both
# trees and compare the minimum of each (min of N): the minimum is the run with
# the least interference from the rest of the machine. Output is one TSV row
# per run: label, run, wall_s, user_s, sys_s, executables, bytes.
set -euo pipefail
label="${1:?label}"; runs="${2:?runs}"; shift 2
crates=("$@")
[ "${#crates[@]}" -gt 0 ] || { echo "no crates named" >&2; exit 2; }
pkgs=(); for c in "${crates[@]}"; do pkgs+=(-p "$c"); done
for run in $(seq 1 "$runs"); do
  cargo clean "${pkgs[@]}" >/dev/null 2>&1
  tmp="$(mktemp)"; msgs="$(mktemp)"
  /usr/bin/time -p cargo test --locked --no-run --message-format=json "${pkgs[@]}" >"$msgs" 2>"$tmp" || { tail -20 "$tmp" >&2; exit 1; }
  wall="$(awk '/^real/ {print $2}' "$tmp")"; user="$(awk '/^user/ {print $2}' "$tmp")"; sys="$(awk '/^sys/ {print $2}' "$tmp")"
  # Executables cargo reports for the named packages' test targets.
  read -r count bytes < <(python3 - "$msgs" "${crates[@]}" <<'PY'
import json, os, sys
msgs, names = sys.argv[1], set(sys.argv[2:])
exes = set()
for line in open(msgs):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") != "compiler-artifact" or not m.get("executable"):
        continue
    pkg = m["package_id"].split("#")[-1].split("@")[0] if "#" in m["package_id"] else m["package_id"].split(" ")[0]
    if pkg in names or any(pkg.startswith(n + "@") or m["package_id"].find("/" + n + "#") >= 0 or m["package_id"].find("/" + n + ")") >= 0 for n in names):
        exes.add(m["executable"])
print(len(exes), sum(os.path.getsize(e) for e in exes))
PY
)
  rm -f "$tmp" "$msgs"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$run" "$wall" "$user" "$sys" "$count" "$bytes"
done
