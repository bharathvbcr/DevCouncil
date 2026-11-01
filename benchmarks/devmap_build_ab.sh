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
#   arm rep kind total extract resolve analyze persist write prune_gen prune_ext vacuum symbols edges unresolved db_bytes
# summary.txt then gives min, median and max per arm and kind. A same-binary
# pair of arms (a=X b=X) measures the noise floor.
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
printf 'arm\trep\tkind\ttotal\textract\tresolve\tanalyze\tpersist\twrite\tprune_gen\tprune_ext\tvacuum\tsymbols\tedges\tunresolved\tdb_bytes\n' > "$TSV"

EDIT1=$(cd "$CORPUS" && git ls-files '*.ts' | head -1)
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
                 sec("persist:prune_extractions"), sec("persist:vacuum"), str(d["symbols"]), str(d["edges"]),
                 str(d["unresolved_calls"]), str(size)]))
PY
}

build() {
  # $1 binary $2 db $3 json $4.. extra flags
  local bin=$1 db=$2 json=$3
  shift 3
  "$bin" build "$CORPUS" --db "$db" --json --progress never "$@" > "$json" 2> "${json%.json}.err"
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
    build "$bin" "$db" "$OUT/$arm-$rep-cold.json" --full && row "$arm" "$rep" cold "$OUT/$arm-$rep-cold.json" "$db"
    printf '\n// ab edit %s\n' "$rep" >> "$CORPUS/$EDIT1"
    build "$bin" "$db" "$OUT/$arm-$rep-inc1.json" && row "$arm" "$rep" inc1 "$OUT/$arm-$rep-inc1.json" "$db"
    printf '\n// ab edit %s\n' "$rep" >> "$CORPUS/$EDIT2"
    build "$bin" "$db" "$OUT/$arm-$rep-inc2.json" && row "$arm" "$rep" inc2 "$OUT/$arm-$rep-inc2.json" "$db"
    (cd "$CORPUS" && git checkout -q -- "$EDIT1" "$EDIT2")
    rm -f "$db" "$db"-wal "$db"-shm "$db".writer.lock
  done
done

python3 -I - "$TSV" > "$OUT/summary.txt" <<'PY'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
cols = ["total", "extract", "resolve", "analyze", "persist", "write", "prune_gen", "vacuum", "db_bytes"]
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
