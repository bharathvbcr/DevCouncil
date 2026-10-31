#!/bin/bash
# DevMap graph-parity and parse-coverage harness.
#
#   devmap_graph_parity.sh <devmap binary> <output dir> <corpus dir>
#
# Cold `build --full` of the frozen GitPulse corpus into a scratch DB under
# <output dir>, then emits, all in <output dir>:
#   summary.txt   (a) degraded_reason + charged partial count
#                 (b) partial_parse listed total and per-language split, plus
#                     the other coverage_gaps kinds
#                 (c) row counts and sha256 of the three ordered dumps
#   nodes.tsv edges.tsv unresolved.tsv   ordered dumps (valid_to IS NULL),
#                 first column is the path so a path-level diff is a `cut`
#   partial.tsv   the listed partial_parse files (path, language)
# The scratch DB is deleted at the end; the dumps are what is kept.
set -u
BIN=$1
OUT=$2
CORPUS=${3:?usage: devmap_graph_parity.sh <devmap binary> <output dir> <corpus dir>}
mkdir -p "$OUT"
DB="$OUT/g.sqlite"
rm -f "$DB" "$DB"-wal "$DB"-shm "$DB".writer.lock

"$BIN" build "$CORPUS" --db "$DB" --full --json --progress never > "$OUT/build.json" 2> "$OUT/build.err"
echo "build exit=$?" > "$OUT/summary.txt"
"$BIN" status --db "$DB" --json > "$OUT/status.json" 2> "$OUT/status.err"

# Text columns are escaped so a tab or a newline inside one cannot split a row.
esc() {
  printf '%s' "replace(replace(replace(replace($1,char(92),char(92,92)),char(10),char(92,110)),char(13),char(92,114)),char(9),char(92,116))"
}

GEN="(SELECT MAX(id) FROM generations)"
TAB=$'\t'

sqlite3 -separator "$TAB" "$DB" "
SELECT $(esc 'p.path'), $(esc 'n.qualified_name'), n.kind, n.span_start, n.span_end, n.is_exported, $(esc 'n.name')
FROM generation_nodes n JOIN paths p ON p.id = n.file_id
WHERE n.generation_id = $GEN
ORDER BY p.path, n.qualified_name, n.kind, n.span_start, n.span_end, n.name, n.is_exported;" > "$OUT/nodes.tsv" 2> "$OUT/nodes.err"

sqlite3 -separator "$TAB" "$DB" "
SELECT $(esc 'ps.path'), $(esc 'e.source_symbol'), $(esc 'pt.path'), $(esc 'e.target_symbol'), e.edge_kind,
       ifnull(e.resolution,''), printf('%.6f', e.confidence), ifnull(e.candidate_total,'')
FROM edge_rows e JOIN paths ps ON ps.id = e.source_file_id JOIN paths pt ON pt.id = e.target_file_id
WHERE e.valid_to IS NULL
ORDER BY ps.path, e.source_symbol, pt.path, e.target_symbol, e.edge_kind, ifnull(e.resolution,''), e.confidence, ifnull(e.candidate_total,'');" > "$OUT/edges.tsv" 2> "$OUT/edges.err"

sqlite3 -separator "$TAB" "$DB" "
SELECT $(esc 'p.path'), $(esc 'u.source_symbol'), $(esc 'u.callee_name'), $(esc "ifnull(u.receiver,'')"),
       $(esc 'r.text'), $(esc 'c.text')
FROM unresolved_rows u JOIN paths p ON p.id = u.source_file_id
JOIN unresolved_texts r ON r.id = u.reason_id JOIN unresolved_texts c ON c.id = u.classification_id
WHERE u.valid_to IS NULL
ORDER BY p.path, u.source_symbol, u.callee_name, ifnull(u.receiver,''), r.text, c.text;" > "$OUT/unresolved.tsv" 2> "$OUT/unresolved.err"

sqlite3 -separator "$TAB" "$DB" "
SELECT g.path, ifnull(f.language,'?')
FROM generation_coverage_gaps g JOIN paths p ON p.path = g.path
LEFT JOIN generation_files f ON f.file_id = p.id AND f.generation_id = g.generation_id
WHERE g.generation_id = $GEN AND g.gap = 'partial_parse'
ORDER BY f.language, g.path;" > "$OUT/partial.tsv" 2> "$OUT/partial.err"

REASON=$(jq -r '.degraded_reason // ""' "$OUT/status.json")
CHARGED=$(printf '%s' "$REASON" | grep -oE '[0-9]+ file[(]s[)] parsed with ERROR' | grep -oE '^[0-9]+')
{
  echo "binary: $("$BIN" --version 2>&1 | head -1)"
  echo "nodes=$(wc -l < "$OUT/nodes.tsv" | tr -d ' ') edges=$(wc -l < "$OUT/edges.tsv" | tr -d ' ') unresolved=$(wc -l < "$OUT/unresolved.tsv" | tr -d ' ')"
  echo "partial_parse listed: $(wc -l < "$OUT/partial.tsv" | tr -d ' ')"
  echo "partial_parse by language: $(cut -f2 "$OUT/partial.tsv" | sort | uniq -c | sort -rn | awk '{printf "%s %s, ", $2, $1}')"
  echo "coverage_gaps kinds (total): $(jq -r '.coverage_gaps | to_entries | map("\(.key)=\(.value.total // "?")") | join(" ")' "$OUT/status.json")"
  echo "charged partial (files parsed with ERROR/MISSING regions): ${CHARGED:-0}"
  echo "degraded_reason: $REASON"
  echo "sha256 nodes:      $(shasum -a 256 < "$OUT/nodes.tsv" | cut -d' ' -f1)"
  echo "sha256 edges:      $(shasum -a 256 < "$OUT/edges.tsv" | cut -d' ' -f1)"
  echo "sha256 unresolved: $(shasum -a 256 < "$OUT/unresolved.tsv" | cut -d' ' -f1)"
} >> "$OUT/summary.txt"

rm -f "$DB" "$DB"-wal "$DB"-shm "$DB".writer.lock
cat "$OUT/summary.txt"
