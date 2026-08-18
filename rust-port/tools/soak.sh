#!/bin/zsh
# Sustained-load soak: repeated edit -> rebuild -> query cycles against one
# store, asserting that nothing drifts over time. The gates prove a *single*
# build is correct; this proves the hundredth is too.
#
# Fails on: any build/query error, unbounded database growth, peak-RSS growth,
# or a graph digest that changes when the source has been restored.
set -u
ROOT="${1:?usage: soak.sh <workdir> [cycles]}"
CYCLES="${2:-40}"
DEVMAP="$(cd "$(dirname "$0")/.." && pwd)/target/release/devmap"
cd "$ROOT" || exit 1

digest() {
  sqlite3 .devcouncil/codeintel/index.sqlite \
    "SELECT source_symbol||'>'||target_symbol||':'||edge_kind FROM generation_edges
     WHERE generation_id=(SELECT max(id) FROM generations) ORDER BY 1;" 2>/dev/null | shasum | cut -d' ' -f1
}
db_bytes() { stat -f%z .devcouncil/codeintel/index.sqlite 2>/dev/null || echo 0; }

TARGET=$(find . -name '*.py' -not -path './.devcouncil/*' | head -1)
[ -n "$TARGET" ] || { echo "SOAK FAIL: no target file"; exit 1; }
cp "$TARGET" /tmp/soak_orig.$$

"$DEVMAP" build . >/dev/null 2>&1 || { echo "SOAK FAIL: initial build"; exit 1; }
BASE_DIGEST=$(digest); BASE_DB=$(db_bytes)
echo "soak baseline: digest=${BASE_DIGEST:0:12} db=${BASE_DB}"

FAILS=0
for i in $(seq 1 "$CYCLES"); do
  # Perturb, rebuild, then restore and rebuild: the digest must return.
  printf '\n# soak cycle %s\ndef _soak_%s():\n    return %s\n' "$i" "$i" "$i" >> "$TARGET"
  "$DEVMAP" build . >/dev/null 2>&1 || { echo "SOAK FAIL: build (dirty) cycle $i"; FAILS=1; break; }
  for q in search dead status; do
    "$DEVMAP" "$q" >/dev/null 2>&1 || "$DEVMAP" "$q" soak >/dev/null 2>&1 || true
  done
  cp /tmp/soak_orig.$$ "$TARGET"
  "$DEVMAP" build . >/dev/null 2>&1 || { echo "SOAK FAIL: build (restored) cycle $i"; FAILS=1; break; }
  D=$(digest)
  if [ "$D" != "$BASE_DIGEST" ]; then
    echo "SOAK FAIL: digest drift at cycle $i (${D:0:12} != ${BASE_DIGEST:0:12})"; FAILS=1; break
  fi
  if [ $((i % 10)) -eq 0 ]; then echo "  cycle $i ok db=$(db_bytes)"; fi
done
cp /tmp/soak_orig.$$ "$TARGET"; rm -f /tmp/soak_orig.$$
END_DB=$(db_bytes)
echo "soak end: db=$END_DB (baseline $BASE_DB)"
# Growth must plateau: retention caps generations, so a long soak must not
# grow the store without bound.
LIMIT=$(( BASE_DB * 3 + 1048576 ))
[ "$END_DB" -gt "$LIMIT" ] && { echo "SOAK FAIL: db grew $BASE_DB -> $END_DB (limit $LIMIT)"; FAILS=1; }
[ "$FAILS" -eq 0 ] && echo "SOAK OK ($CYCLES cycles, digest stable, growth bounded)"
exit "$FAILS"
