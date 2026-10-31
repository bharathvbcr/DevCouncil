#!/usr/bin/env bash
# Fuse and score every arm for every view. Inputs are the per-arm run files
# under $RUNS (named <set>.<arm>.<condition>.jsonl, set = commit|stranger,
# arm = A|B|C, condition = W|P); outputs are written to $OUT.
#
#   RE=path/to/rank-eval RUNS=dir OUT=dir benchmarks/retrieval-eval/score.sh
#
# Arms: A = BM25 code-v1, B = learned sparse (doc-v2-mini), C = dense MiniLM,
# D = RRF(A, C), AB = RRF(A, B), ABC = RRF(A, B, C), all k = 60.
# Condition W is the whole repository; P is the stock dcgrep's truncated
# index. B and C have no corpus-dependent statistics, so their P rankings are
# their W runs filtered to P's files (the fuse and score steps do the filtering).
set -euo pipefail
: "${RE:?path to rank-eval}" "${RUNS:?run directory}" "${OUT:?output directory}"
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
EXCL=(--exclude-prefix docs/ --exclude-suffix .md)

view() { # name set condition queries corpus [exclusions...]
  local name=$1 set=$2 cond=$3 queries=$4 corpus=$5
  shift 5
  local dir="$OUT/$name"
  mkdir -p "$dir"
  local a="$RUNS/$set.A.$cond.jsonl" b="$RUNS/$set.B.W.jsonl" c="$RUNS/$set.C.W.jsonl"
  # fuse_one writes into $dir; the lists are corpus-filtered before fusing.
  "$RE" fuse --corpus "$corpus" "$@" --out "$dir/D.jsonl" "$a" "$c"
  "$RE" fuse --corpus "$corpus" "$@" --out "$dir/AB.jsonl" "$a" "$b"
  "$RE" fuse --corpus "$corpus" "$@" --out "$dir/ABC.jsonl" "$a" "$b" "$c"
  "$RE" score --queries "$queries" --corpus "$corpus" "$@" --label "$name" \
    --baseline A --arm "A=$a" --arm "B=$b" --arm "C=$c" \
    --arm "D=$dir/D.jsonl" --arm "AB=$dir/AB.jsonl" --arm "ABC=$dir/ABC.jsonl" \
    --resamples 5000 --json "$dir/scores.json" | tee "$dir/scores.md"
  # The same arms against learned-sparse, for "does dense beat B" and
  # "does A+C beat A+B".
  "$RE" score --queries "$queries" --corpus "$corpus" "$@" --label "$name (baseline B)" \
    --baseline B --arm "A=$a" --arm "B=$b" --arm "C=$c" \
    --arm "D=$dir/D.jsonl" --arm "AB=$dir/AB.jsonl" --arm "ABC=$dir/ABC.jsonl" \
    --resamples 5000 --json "$dir/scores-vsB.json" > "$dir/scores-vsB.md"
  # And against BM25+learned-sparse fusion, for "is the fused gain dense's
  # or fusion's" (D - AB).
  "$RE" score --queries "$queries" --corpus "$corpus" "$@" --label "$name (baseline AB)" \
    --baseline AB --arm "A=$a" --arm "B=$b" --arm "C=$c" \
    --arm "D=$dir/D.jsonl" --arm "AB=$dir/AB.jsonl" --arm "ABC=$dir/ABC.jsonl" \
    --resamples 5000 --json "$dir/scores-vsAB.json" > "$dir/scores-vsAB.md"
}

C="$HERE/commit-subjects.jsonl"
S="$HERE/stranger-labeled.jsonl"
view W-commit commit W "$C" "$HERE/corpus-W.txt"
view W-stranger-nodocs stranger W "$S" "$HERE/corpus-W.txt" "${EXCL[@]}"
view W-stranger-all stranger W "$S" "$HERE/corpus-W.txt"
view P-commit commit P "$C" "$HERE/corpus-P.txt"
view P-stranger-nodocs stranger P "$S" "$HERE/corpus-P.txt" "${EXCL[@]}"
view P-stranger-all stranger P "$S" "$HERE/corpus-P.txt"
