#!/bin/bash
# Failure accounting of devmap_build_ab.sh.
#
#   test_devmap_build_ab.sh
#
# Runs the harness over a throwaway git corpus with two stub "binaries":
#   ok    prints a minimal valid `build --json` result and exits 0;
#   bad   does the same except that its cold build (`--full`) exits 1.
# A build that exits non-zero used to vanish from runs.tsv and leave the exit
# status 0, so a benchmark with a broken arm summarised the rows that survived
# as if nothing had happened. This asserts that the harness now
#   - exits non-zero,
#   - names the failed build (arm, rep, kind, exit code, .err path),
#   - still writes runs.tsv rows and summary.txt for the builds that succeeded,
#   - and exits 0 when no build failed.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
HARNESS="$HERE/devmap_build_ab.sh"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/devmap-build-ab-test.XXXXXX") || exit 2
trap 'rm -rf "$WORK"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

CORPUS="$WORK/corpus"
mkdir -p "$CORPUS"
(
  cd "$CORPUS" || exit 1
  git init -q . &&
    printf 'export const a = 1;\n' > a.ts &&
    printf 'fn main() {}\n' > b.rs &&
    git add a.ts b.rs &&
    git -c user.name=t -c user.email=t@example.com commit -q -m corpus
) || fail "could not create the corpus"

cat > "$WORK/stub-ok" <<'STUB'
#!/bin/bash
echo '{"timings":{"total_seconds":0.5,"stages":[]},"symbols":3,"edges":2,"unresolved_calls":1}'
STUB
cat > "$WORK/stub-bad" <<'STUB'
#!/bin/bash
for a in "$@"; do
  if [ "$a" = "--full" ]; then
    echo "stub: refusing the cold build" >&2
    exit 1
  fi
done
echo '{"timings":{"total_seconds":0.5,"stages":[]},"symbols":3,"edges":2,"unresolved_calls":1}'
STUB
chmod +x "$WORK/stub-ok" "$WORK/stub-bad"

# One failing build among six: the harness must say so and exit non-zero.
bash "$HARNESS" "$WORK/out" "$CORPUS" 1 "ok=$WORK/stub-ok" "bad=$WORK/stub-bad" > "$WORK/stdout" 2> "$WORK/stderr"
rc=$?
[ "$rc" -ne 0 ] || fail "exit status was 0 with a failed build"
cat "$WORK/stdout" "$WORK/stderr" > "$WORK/all"
grep -Eq 'bad.*rep 1.*cold.*exit 1.*bad-1-cold\.err' "$WORK/all" ||
  fail "the failed build was not named (arm, rep, kind, exit code, .err path); output was: $(cat "$WORK/all")"
grep -Eq '1 build\(s\) failed' "$WORK/all" || fail "no failure count in the output"

[ -s "$WORK/out/summary.txt" ] || fail "summary.txt was not written"
rows=$(($(wc -l < "$WORK/out/runs.tsv") - 1))
[ "$rows" -eq 5 ] || fail "expected 5 runs.tsv rows (6 builds, 1 failed), got $rows"
if awk -F'\t' '$1 == "bad" && $3 == "cold"' "$WORK/out/runs.tsv" | grep -q .; then
  fail "the failed build left a row in runs.tsv"
fi

# Nothing failing: exit 0 and no failure report.
bash "$HARNESS" "$WORK/out-ok" "$CORPUS" 1 "ok=$WORK/stub-ok" "ok2=$WORK/stub-ok" > "$WORK/stdout2" 2> "$WORK/stderr2"
rc=$?
[ "$rc" -eq 0 ] || fail "exit status was $rc with no failed build"
if grep -q 'failed' "$WORK/stdout2" "$WORK/stderr2"; then
  fail "a failure was reported with no failed build"
fi

echo "PASS: devmap_build_ab.sh accounts for failed builds"
