#!/bin/zsh
# devmap_graph_diff.sh <dump dir A> <dump dir B>
# Paths whose node / edge / unresolved rows differ between two harness runs,
# with the added (+) and removed (-) row counts per path and dump.
set -u
A=$1
B=$2
for kind in nodes edges unresolved; do
  echo "== $kind: $(wc -l < "$A/$kind.tsv" | tr -d ' ') -> $(wc -l < "$B/$kind.tsv" | tr -d ' ') rows; sha A $(shasum -a 256 < "$A/$kind.tsv" | cut -c1-12) B $(shasum -a 256 < "$B/$kind.tsv" | cut -c1-12)"
  LC_ALL=C diff <(LC_ALL=C sort "$A/$kind.tsv") <(LC_ALL=C sort "$B/$kind.tsv") \
    | LC_ALL=C awk -F'\t' '/^< /{sub(/^< /,"",$1); r[$1]=r[$1]; m[$1]++} /^> /{sub(/^> /,"",$1); p[$1]++; r[$1]=r[$1]} END{for(k in r) printf "%s\t+%d\t-%d\n", k, p[k]+0, m[k]+0}' \
    | LC_ALL=C sort
done
