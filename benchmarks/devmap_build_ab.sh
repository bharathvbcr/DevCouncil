#!/bin/bash
# Interleaved A/B timing of `devmap build` on a frozen corpus.
#
#   devmap_build_ab.sh <out dir> <corpus dir> <reps> <label=binary> [<label=binary> ...]
#
# Each rep runs every arm in turn (ABAB..., the order rotated each rep so no arm
# always goes first) through three builds into its own scratch DB:
#   cold    `build --full`                         (1 generation)
#   inc1    one file changed                       (1 -> 2 generations; no prune)
#   inc2    another file changed                   (2 -> 3, prunes back to 2: steady state)
# Every build's `--json` result is kept as <out>/<arm>-<rep>-<kind>.json and one
# row per build lands in <out>/runs.tsv:
#   arm rep kind total extract resolve analyze persist write prune_gen prune_ext vacuum artifacts art_status art_freshness art_load art_render art_write art_stamp symbols edges unresolved db_bytes
# AB_MANIFEST=1 adds --manifest to every build, so the "writing consumer
# artifacts" stage and its artifacts:* phases are measured (a build without the
# flag writes none and those columns are empty). A binary that predates the
# phases leaves the art_* columns empty and still reports the stage total.
# summary.txt then gives min, median and max per arm and kind. A same-binary
# pair of arms (a=X b=X) measures the noise floor.
#
# A build that exits non-zero (or whose --json result cannot be read) writes no
# row. Each one is reported on stderr as it happens (arm, rep, kind, exit code,
# the .err file holding the build's stderr), summary.txt is still written for
# the builds that succeeded, and the script lists every failure again at the end
# and exits 1. Exit 0 therefore means every row of every arm is in runs.tsv.
# benchmarks/test_devmap_build_ab.sh pins this with stub binaries.
#
# The corpus is a git repository; the edited files are restored with `git
# checkout` after each rep, so every rep starts from the same tree.
set -u
OUT=$1
CORPUS=$2
REPS=$3
shift 3
mkdir -p "$OUT"
TSV="$OUT/runs.tsv"
printf 'arm\trep\tkind\ttotal\textract\tresolve\tanalyze\tpersist\twrite\tprune_gen\tprune_ext\tvacuum\tartifacts\tart_status\tart_freshness\tart_load\tart_render\tart_write\tart_stamp\tsymbols\tedges\tunresolved\tdb_bytes\n' > "$TSV"
MANIFEST_FLAG=""
if [ "${AB_MANIFEST:-0}" = 1 ]; then MANIFEST_FLAG="--manifest"; fi

EDIT1=$(cd "$CORPUS" && git ls-files '*.ts' | head -1)
# A corpus with no TypeScript edits its first Go file instead, so the harness
# does not append to the corpus directory itself.
if [ -z "$EDIT1" ]; then EDIT1=$(cd "$CORPUS" && git ls-files '*.go' | head -1); fi
if [ -z "$EDIT1" ]; then echo "no .ts or .go file in $CORPUS to edit" >&2; exit 2; fi
EDIT2=$(cd "$CORPUS" && git ls-files '*.rs' | head -1)

row() {
  # $1 arm $2 rep $3 kind $4 json $5 db
  python3 -I - "$1" "$2" "$3" "$4" "$5" >> "$TSV" <<'PY'
import json, os, sys
arm, rep, kind, path, db = sys.argv[1:]
d = json.load(open(path))
t = d["timings"]
def find(stages, prefix):
    for s in stages:
        if s["stage"].startswith(prefix):
            return s
        hit = find(s.get("sub", []), prefix)
        if hit:
            return hit
    return None
def sec(prefix):
    s = find(t["stages"], prefix)
    return "%.4f" % s["seconds"] if s else ""
size = sum(os.path.getsize(db + x) for x in ("", "-wal") if os.path.exists(db + x))
print("\t".join([arm, rep, kind, "%.4f" % t["total_seconds"], sec("extract:files"), sec("resolving"),
                 sec("analyzing"), sec("persisting"), sec("persist:write"), sec("persist:prune_generations"),
                 sec("persist:prune_extractions"), sec("persist:vacuum"),
                 sec("writing consumer artifacts"), sec("artifacts:status"), sec("artifacts:freshness"),
                 sec("artifacts:load"), sec("artifacts:render"), sec("artifacts:write"),
                 sec("artifacts:stamp_write"), str(d["symbols"]), str(d["edges"]),
                 str(d["unresolved_calls"]), str(size)]))
PY
}

build() {
  # $1 binary $2 db $3 json $4.. extra flags
  local bin=$1 db=$2 json=$3
  shift 3
  "$bin" build "$CORPUS" --db "$db" --json --progress never ${MANIFEST_FLAG:+"$MANIFEST_FLAG"} "$@" > "$json" 2> "${json%.json}.err"
}

FAILURES=()
step() {
  # $1 arm $2 rep $3 kind $4 binary $5 db $6.. extra flags
  # One build, then its runs.tsv row. A build that exits non-zero, or whose
  # result `row` cannot read, contributes no row and is recorded for the report
  # at the end: a missing row must never look like a build that was not run.
  local arm=$1 rep=$2 kind=$3 bin=$4 db=$5 json rc
  shift 5
  json="$OUT/$arm-$rep-$kind.json"
  build "$bin" "$db" "$json" "$@"
  rc=$?
  if ((rc != 0)); then
    FAILURES+=("arm $arm rep $rep kind $kind: build exit $rc, stderr in ${json%.json}.err")
    echo "FAILED: ${FAILURES[${#FAILURES[@]} - 1]}" >&2
    return 0
  fi
  if ! row "$arm" "$rep" "$kind" "$json" "$db"; then
    FAILURES+=("arm $arm rep $rep kind $kind: build exit 0 but its result $json could not be read, stderr in ${json%.json}.err")
    echo "FAILED: ${FAILURES[${#FAILURES[@]} - 1]}" >&2
  fi
  return 0
}

ARMS=("$@")
N=${#ARMS[@]}
for ((rep = 1; rep <= REPS; rep++)); do
  for ((k = 0; k < N; k++)); do
    spec=${ARMS[$(((k + rep) % N))]}
    arm=${spec%%=*}
    bin=${spec#*=}
    db="$OUT/$arm-$rep.sqlite"
    rm -f "$db" "$db"-wal "$db"-shm "$db".writer.lock
    step "$arm" "$rep" cold "$bin" "$db" --full
    printf '\n// ab edit %s\n' "$rep" >> "$CORPUS/$EDIT1"
    step "$arm" "$rep" inc1 "$bin" "$db"
    printf '\n// ab edit %s\n' "$rep" >> "$CORPUS/$EDIT2"
    step "$arm" "$rep" inc2 "$bin" "$db"
    (cd "$CORPUS" && git checkout -q -- "$EDIT1" "$EDIT2")
    rm -f "$db" "$db"-wal "$db"-shm "$db".writer.lock
  done
done

python3 -I - "$TSV" > "$OUT/summary.txt" <<'PY'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
cols = ["total", "extract", "resolve", "analyze", "persist", "write", "prune_gen", "vacuum", "artifacts", "art_status", "art_freshness", "art_load", "art_render", "art_write", "art_stamp", "db_bytes"]
groups = {}
for r in rows:
    groups.setdefault((r["kind"], r["arm"]), []).append(r)
print("kind\tarm\tn\t" + "\t".join(c + "(min/med/max)" for c in cols) + "\tsymbols/edges/unresolved")
for (kind, arm), rs in sorted(groups.items()):
    cells = []
    for c in cols:
        v = [float(r[c]) for r in rs if r[c]]
        if not v:
            cells.append("-")
        elif c == "db_bytes":
            cells.append("%.1f/%.1f/%.1f MB" % (min(v) / 1e6, statistics.median(v) / 1e6, max(v) / 1e6))
        else:
            cells.append("%.3f/%.3f/%.3f" % (min(v), statistics.median(v), max(v)))
    counts = sorted({(r["symbols"], r["edges"], r["unresolved"]) for r in rs})
    print("%s\t%s\t%d\t%s\t%s" % (kind, arm, len(rs), "\t".join(cells), counts))
PY
cat "$OUT/summary.txt"

if ((${#FAILURES[@]} > 0)); then
  echo >&2
  echo "${#FAILURES[@]} build(s) failed; their rows are missing from runs.tsv and summary.txt covers only the builds that succeeded:" >&2
  printf '  %s\n' "${FAILURES[@]}" >&2
  exit 1
fi
