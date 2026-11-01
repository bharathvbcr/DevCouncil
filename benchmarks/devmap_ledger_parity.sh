#!/bin/bash
# Ledger parity across one incremental build.
#
#   devmap_ledger_parity.sh <devmap binary> <output dir> <corpus dir>
#
# devmap_graph_parity.sh hashes the live graph after a cold build. A change to
# how the unresolved ledger is written (id assignment in `unresolved_texts`,
# the row inserts, the close-and-reinsert comparison) can leave that hash
# intact and still shift ids or history, so this script also:
#   1. cold-builds the corpus (`build --full`) into a scratch DB,
#   2. appends a comment to one tracked .ts file and builds again (one
#      incremental generation),
#   3. dumps `unresolved_texts` ordered by id and every `unresolved_rows` row,
#      live and closed, with its generation bounds, ordered by content.
# Run it with two binaries and `cmp` the dumps (texts.tsv, rows-all.tsv).
#
# The corpus is edited in place: pass a private copy of the frozen corpus (or
# restore the file with `git checkout`). The scratch DB is deleted at the end.
set -u
BIN=$1
OUT=$2
CORPUS=${3:?usage: devmap_ledger_parity.sh <devmap binary> <output dir> <corpus dir>}
mkdir -p "$OUT"
DB="$OUT/ledger.sqlite"
rm -f "$DB" "$DB"-wal "$DB"-shm "$DB".writer.lock

EDIT=$(cd "$CORPUS" && git ls-files '*.ts' | head -1)
"$BIN" build "$CORPUS" --db "$DB" --full --json --progress never > "$OUT/cold.json" 2> "$OUT/cold.err"
printf '\n// ledger parity edit\n' >> "$CORPUS/$EDIT"
"$BIN" build "$CORPUS" --db "$DB" --json --progress never > "$OUT/inc1.json" 2> "$OUT/inc1.err"

sqlite3 "$DB" "SELECT id, text FROM unresolved_texts ORDER BY id;" > "$OUT/texts.tsv"
sqlite3 "$DB" "SELECT count(*), sum(valid_to IS NULL) FROM unresolved_rows;" > "$OUT/rows-count.txt"
sqlite3 "$DB" "
SELECT p.path, u.source_symbol, u.callee_name, ifnull(u.receiver,''), u.reason_id, u.classification_id, u.valid_from, u.valid_to
FROM unresolved_rows u JOIN paths p ON p.id = u.source_file_id
ORDER BY p.path, u.source_symbol, u.callee_name, ifnull(u.receiver,''), u.reason_id, u.classification_id, u.valid_from;" > "$OUT/rows-all.tsv"
shasum -a 256 "$OUT/texts.tsv" "$OUT/rows-all.tsv" > "$OUT/ledger.sha"
(cd "$CORPUS" && git checkout -q -- "$EDIT")
rm -f "$DB" "$DB"-wal "$DB"-shm "$DB".writer.lock
cat "$OUT/rows-count.txt" "$OUT/ledger.sha"
