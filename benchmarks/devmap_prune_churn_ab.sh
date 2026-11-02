#!/bin/bash
# Interleaved A/B of `devmap build` through a rename, which is where
# `persist:prune_generations` is expensive.
#
#   devmap_prune_churn_ab.sh <out dir> <corpus clone> <reps> <label=binary> [<label=binary> ...]
#
# <corpus clone> must be a PRIVATE clone of the frozen corpus: the script moves
# files in it and resets it with `git reset --hard` / `git clean` after every
# run, so never point it at a shared corpus. Each rep runs every arm (order
# rotated per rep) through
#   cold   `build --full`                         (generation 1)
#   inc1   `git mv src/lib src/lib_r`, build      (generation 2: the rename closes
#                                                  and reopens every ledger row)
#   inc2   one more edit, build                   (generation 3: prunes back to
#                                                  2, retiring the old paths and
#                                                  texts)
# and records, for inc2, the stage timings and the end-state row counts, plus
# `PRAGMA foreign_key_check` on the finished store.
#
# BUILD_CAP (seconds, default 900) bounds every build. A build that hits it is
# recorded as `capped` and its time is a LOWER BOUND, not a measurement.
set -u
OUT=$1
CORPUS=$2
REPS=$3
shift 3
CAP=${BUILD_CAP:-900}
mkdir -p "$OUT"
TSV="$OUT/runs.tsv"
printf 'arm\trep\tstatus\ttotal\tpersist\tprune_gen\tvacuum\tgenerations\tedge_rows\tunresolved_rows\tfile_rows\tdigest_rows\tfile_payloads\tpaths\tunresolved_texts\tfk_violations\tdb_bytes\n' > "$TSV"

row() {
  # $1 arm $2 rep $3 status $4 json $5 db
  local arm=$1 rep=$2 status=$3 json=$4 db=$5
  local timings="" counts="" fk="" size=0
  if [ -s "$json" ]; then
    timings=$(python3 -I - "$json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
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
print("\t".join(["%.4f" % t["total_seconds"], sec("persisting"), sec("persist:prune_generations"), sec("persist:vacuum")]))
PY
)
  else
    # A capped build wrote no result: record the cap as the total, a lower bound.
    if [ "$status" = capped ]; then timings="$CAP"$'\t\t\t'; else timings=$'\t\t\t'; fi
  fi
  if [ -f "$db" ]; then
    counts=$(sqlite3 "$db" "SELECT (SELECT count(*) FROM generations), (SELECT count(*) FROM edge_rows), (SELECT count(*) FROM unresolved_rows), (SELECT count(*) FROM file_rows), (SELECT count(*) FROM digest_rows), (SELECT count(*) FROM file_payloads), (SELECT count(*) FROM paths), (SELECT count(*) FROM unresolved_texts);" | tr '|' '\t')
    fk=$(sqlite3 "$db" "SELECT count(*) FROM pragma_foreign_key_check;")
    size=$(($(stat -f %z "$db") + $( [ -f "$db-wal" ] && stat -f %z "$db-wal" || echo 0)))
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$arm" "$rep" "$status" "$timings" "$counts" "$fk" "$size" >> "$TSV"
}

build() {
  # $1 binary $2 db $3 json $4.. extra flags; exit 124 when capped
  local bin=$1 db=$2 json=$3
  shift 3
  timeout "$CAP" "$bin" build "$CORPUS" --db "$db" --json --progress never "$@" > "$json" 2> "${json%.json}.err"
}

reset_corpus() {
  (cd "$CORPUS" && git reset -q --hard HEAD && git clean -fdq)
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
    reset_corpus
    status=ok
    build "$bin" "$db" "$OUT/$arm-$rep-cold.json" --full || status=cold-failed
    if [ "$status" = ok ]; then
      (cd "$CORPUS" && git mv src/lib src/lib_r)
      build "$bin" "$db" "$OUT/$arm-$rep-inc1.json" || status=inc1-failed
    fi
    if [ "$status" = ok ]; then
      printf '\n// churn edit %s\n' "$rep" >> "$CORPUS/src/main.ts"
      build "$bin" "$db" "$OUT/$arm-$rep-inc2.json"
      case $? in 0) ;; 124) status=capped ;; *) status=inc2-failed ;; esac
    fi
    row "$arm" "$rep" "$status" "$OUT/$arm-$rep-inc2.json" "$db"
    reset_corpus
    rm -f "$db" "$db"-wal "$db"-shm "$db".writer.lock
  done
done

python3 -I - "$TSV" > "$OUT/summary.txt" <<'PY'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
groups = {}
for r in rows:
    groups.setdefault(r["arm"], []).append(r)
print("arm\tn\tstatus\tprune_gen(min/med/max)\tpersist(min/med/max)\ttotal(min/med/max)\tend-state counts")
for arm, rs in sorted(groups.items()):
    def cell(c):
        v = [float(r[c]) for r in rs if r[c]]
        return "%.3f/%.3f/%.3f" % (min(v), statistics.median(v), max(v)) if v else "-"
    counts = sorted({tuple(r[c] for c in ("generations", "edge_rows", "unresolved_rows", "file_rows",
                                           "digest_rows", "file_payloads", "paths", "unresolved_texts", "fk_violations"))
                     for r in rs})
    print("%s\t%d\t%s\t%s\t%s\t%s\t%s" % (arm, len(rs), ",".join(sorted({r["status"] for r in rs})),
                                         cell("prune_gen"), cell("persist"), cell("total"), counts))
PY
cat "$OUT/summary.txt"
