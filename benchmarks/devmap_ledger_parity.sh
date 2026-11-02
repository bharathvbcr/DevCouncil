#!/bin/bash
# Ledger parity across one incremental build, closed rows included.
#
#   devmap_ledger_parity.sh <devmap binary> <output dir> <corpus dir>
#
# devmap_graph_parity.sh hashes the live graph after a cold build. A change to
# how the unresolved ledger is written (id assignment in `unresolved_texts`,
# the row inserts, the close-and-reinsert comparison) can leave that hash
# intact and still shift ids or history, so this script also:
#   1. cold-builds the corpus (`build --full`) into a scratch DB,
#   2. edits one tracked .ts file so that its unresolved calls change, and builds
#      again (one incremental generation). Every `.push(` in the file becomes
#      `.pushed(`, which closes the rows for the callee `push` and opens rows for
#      `pushed` while the file's other rows are carried; and one call to an
#      undefined function is appended, which opens a row of its own. A comment
#      appended to the file would change nothing the ledger holds: the file's
#      rows would be carried as unchanged and the close-and-reinsert comparison
#      (`close_rows` in devmap-store's write.rs) would never run,
#   3. dumps `unresolved_texts` ordered by id and every `unresolved_rows` row,
#      live and closed, with its `unresolved_id` and generation bounds, ordered by
#      content then id. The id is part of the dump: two stores that agree on
#      every field but number the rows differently are not the same ledger.
# Run it with two binaries and `cmp` the dumps (texts.tsv, rows-all.tsv).
#
# It then counts the rows and exits 1 if the run did not exercise what it
# claims to compare: the count line (rows-count.txt, also printed) gives
#   total   every row, live and closed
#   live    valid_to IS NULL
#   closed  valid_to IS NOT NULL: rows the incremental generation closed
#   opened  rows whose valid_from is past the cold generation's
# and the run fails unless live, closed and opened are all > 0, and the edited
# file has both a closed and a live row. A dump with closed == 0 compared only
# live rows, and `cmp` of two such dumps proves nothing about closing.
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
sed 's/\.push(/.pushed(/g' "$CORPUS/$EDIT" > "$CORPUS/$EDIT.tmp" && mv "$CORPUS/$EDIT.tmp" "$CORPUS/$EDIT"
printf '\nexport const ledgerParityProbe = () => ledgerParityUndefinedCall();\n' >> "$CORPUS/$EDIT"
"$BIN" build "$CORPUS" --db "$DB" --json --progress never > "$OUT/inc1.json" 2> "$OUT/inc1.err"

sqlite3 "$DB" "SELECT id, text FROM unresolved_texts ORDER BY id;" > "$OUT/texts.tsv"
sqlite3 "$DB" "
SELECT u.unresolved_id, p.path, u.source_symbol, u.callee_name, ifnull(u.receiver,''), u.reason_id, u.classification_id, u.valid_from, u.valid_to
FROM unresolved_rows u JOIN paths p ON p.id = u.source_file_id
ORDER BY p.path, u.source_symbol, u.callee_name, ifnull(u.receiver,''), u.reason_id, u.classification_id, u.valid_from, u.unresolved_id;" > "$OUT/rows-all.tsv"
sqlite3 -separator ' ' "$DB" "
SELECT 'total=' || count(*), 'live=' || ifnull(sum(valid_to IS NULL), 0), 'closed=' || ifnull(sum(valid_to IS NOT NULL), 0),
       'opened=' || ifnull(sum(valid_from > (SELECT min(valid_from) FROM unresolved_rows)), 0)
FROM unresolved_rows;" > "$OUT/rows-count.txt"
EDIT_CLOSED=$(sqlite3 "$DB" "SELECT count(*) FROM unresolved_rows u JOIN paths p ON p.id = u.source_file_id WHERE p.path = '$EDIT' AND u.valid_to IS NOT NULL;")
EDIT_LIVE=$(sqlite3 "$DB" "SELECT count(*) FROM unresolved_rows u JOIN paths p ON p.id = u.source_file_id WHERE p.path = '$EDIT' AND u.valid_to IS NULL;")
shasum -a 256 "$OUT/texts.tsv" "$OUT/rows-all.tsv" > "$OUT/ledger.sha"
(cd "$CORPUS" && git checkout -q -- "$EDIT")
rm -f "$DB" "$DB"-wal "$DB"-shm "$DB".writer.lock
cat "$OUT/rows-count.txt" "$OUT/ledger.sha"
echo "edited file $EDIT: closed=$EDIT_CLOSED live=$EDIT_LIVE"

STATUS=0
read -r TOTAL LIVE CLOSED OPENED < "$OUT/rows-count.txt"
for field in "$LIVE" "$CLOSED" "$OPENED"; do
  case $field in
    live=0 | closed=0 | opened=0)
      echo "FAIL: ${field%=0} == 0: the incremental generation did not exercise that part of the ledger, so the dumps do not cover it" >&2
      STATUS=1
      ;;
  esac
done
case ${EDIT_CLOSED:-0} in 0) echo "FAIL: no closed row in $EDIT: the edit closed nothing" >&2; STATUS=1 ;; esac
case ${EDIT_LIVE:-0} in 0) echo "FAIL: no live row in $EDIT: the edit left the file with no rows" >&2; STATUS=1 ;; esac
exit $STATUS
