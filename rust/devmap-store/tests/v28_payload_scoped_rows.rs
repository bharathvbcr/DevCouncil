//! Schema v28: symbols, literals and full-text rows belong to a payload, and a
//! file's membership and digests live for a range of generations.
//!
//! The row-count assertion the card asks for lives in `write_breakdown.rs`
//! (`a_one_file_edit_writes_one_files_worth_of_rows_to_every_relation`) and in
//! the CLI's `b3_one_file_edit_write_set_…`. This file holds the properties a
//! payload-keyed layout can get wrong that a row count cannot see:
//!
//! * a migrated store reads what the store it was migrated from read;
//! * a migration that fails part-way leaves the v27 store as it was;
//! * a payload reached again -- a revert, or a second file with the same bytes --
//!   is neither duplicated nor misreported;
//! * retention collects a payload, with everything derived from it, exactly when
//!   no retained range names it;
//! * a reader running against a writer, or against the retention prune, sees one
//!   generation and never a mixture;
//! * the edges of a file: 50,000 literals, no symbols, no bytes, no grammar.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store, CURRENT_SCHEMA_VERSION, MIN_READER_SCHEMA_VERSION};
use rusqlite::Connection;

#[path = "support/v27_shape.rs"]
mod v27_shape;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-v28-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

type Files = Vec<(String, String)>;

fn commit(store: &Store, files: &Files, affected: &[&str], deleted: &[&str]) -> u32 {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    commit_extractions(store, &extractions, affected, deleted)
}

fn commit_extractions(
    store: &Store,
    extractions: &[devmap_extract::Extraction],
    affected: &[&str],
    deleted: &[&str],
) -> u32 {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation_with_opts(
            extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: affected.iter().map(|path| path.to_string()).collect(),
                deleted_paths: deleted.iter().map(|path| path.to_string()).collect(),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap()
}

fn module(index: usize, revision: usize) -> (String, String) {
    (
        format!("pkg/mod_{index}.py"),
        format!(
            "LABEL_{index} = \"label-{index}\"\n\n\ndef leaf_{index}():\n    return {revision}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"
        ),
    )
}

fn tree(modules: usize, revision_of: impl Fn(usize) -> usize) -> Files {
    (0..modules)
        .map(|index| module(index, revision_of(index)))
        .collect()
}

fn count(db: &Path, sql: &str) -> i64 {
    Connection::open(db)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

/// Everything one generation reads through the compatibility views, by path
/// text so the ids a store happens to have handed out do not matter.
fn read_generation(conn: &Connection, generation: u32) -> Vec<String> {
    let queries = [
        "SELECT 'node|' || p.path || '|' || n.name || '|' || n.qualified_name || '|' || n.kind
                || '|' || n.span_start || '|' || n.span_end || '|' || n.is_exported
                || '|' || COALESCE(n.body_exact, '') || '|' || COALESCE(n.body_nodes, '')
           FROM generation_nodes n JOIN paths p ON p.id = n.file_id
          WHERE n.generation_id = ?1",
        "SELECT 'literal|' || p.path || '|' || l.line || '|' || l.span_start || '|' || l.value
                || '|' || l.qualified_name || '|' || l.symbol_name
           FROM generation_literals l JOIN paths p ON p.id = l.file_id
          WHERE l.generation_id = ?1",
        "SELECT 'file|' || p.path || '|' || f.content_hash || '|' || f.language
           FROM generation_files f JOIN paths p ON p.id = f.file_id
          WHERE f.generation_id = ?1",
        "SELECT 'digest|' || p.path || '|' || d.edge_rows || '|' || d.edge_lo || '|'
                || d.edge_hi || '|' || d.unresolved_rows || '|' || d.unresolved_lo
                || '|' || d.unresolved_hi
           FROM generation_file_digests d JOIN paths p ON p.id = d.file_id
          WHERE d.generation_id = ?1",
        "SELECT 'fts|' || f.name || '|' || f.qualified_name || '|' || f.path
           FROM nodes_fts_map m JOIN nodes_fts f ON f.rowid = m.rowid_ref
          WHERE m.generation_id = ?1",
    ];
    let mut rows = Vec::new();
    for sql in queries {
        let mut stmt = conn.prepare(sql).unwrap();
        rows.extend(
            stmt.query_map([generation], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap),
        );
    }
    rows.sort();
    rows
}

fn all_generations(db: &Path) -> Vec<(u32, Vec<String>)> {
    let conn = Connection::open(db).unwrap();
    let ids: Vec<u32> = conn
        .prepare("SELECT id FROM generations ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    ids.into_iter()
        .map(|id| (id, read_generation(&conn, id)))
        .collect()
}

/// Every payload's identity and digest, by path text.
fn payload_digests(db: &Path) -> Vec<(String, i64, Vec<u8>)> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT p.path, fp.content_hash, fp.payload_digest
               FROM file_payloads fp JOIN paths p ON p.id = fp.file_id
              ORDER BY p.path, fp.content_hash",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

/// A history with every shape a migration has to carry: files that never change,
/// files edited once and twice, a file reverted to content it held before, and a
/// file deleted.
fn history(store: &Store) -> Vec<u32> {
    let mut generations = Vec::new();
    let base = tree(8, |_| 0);
    generations.push(commit(store, &base, &[], &[]));
    let edited = tree(8, |index| usize::from(index == 1));
    generations.push(commit(store, &edited, &["pkg/mod_1.py"], &[]));
    let edited_again = tree(8, |index| {
        if index == 1 {
            2
        } else {
            usize::from(index == 2)
        }
    });
    generations.push(commit(
        store,
        &edited_again,
        &["pkg/mod_1.py", "pkg/mod_2.py"],
        &[],
    ));
    // mod_1 back to what it held in generation 1: its first payload is still
    // stored, and the range that names it is a new one.
    let reverted = tree(8, |index| usize::from(index == 2));
    generations.push(commit(store, &reverted, &["pkg/mod_1.py"], &[]));
    let without: Files = reverted
        .iter()
        .filter(|(path, _)| path != "pkg/mod_7.py")
        .cloned()
        .collect();
    generations.push(commit(store, &without, &[], &["pkg/mod_7.py"]));
    generations
}

/// **Migration identity.** A v27 store opens under this binary, is migrated, and
/// every generation it holds reads exactly what it read before the migration --
/// nodes, literals, membership, digests and full-text rows -- and lists its
/// symbols in the order v27 wrote them.
///
/// The v27 store is this build's own, reduced by `REDUCE_V28_TO_V27`: the flat
/// per-generation tables a v27 binary would have left, filled from what the
/// views read. That makes the "before" the v27 answer by construction, and what
/// is under test is the one thing the reduction does not share with the
/// migration -- the conversion of flat rows into payload rows and maximal
/// ranges, over a history that includes a revert and a deletion.
#[test]
fn a_v27_store_migrates_with_every_generation_reading_the_same_rows() {
    let dir = tmp_dir("migration-identity");
    let db = dir.join("devmap.sqlite");
    let (expected, expected_order, ranges) = {
        let store = Store::open(&db).unwrap();
        history(&store);
        let order: Vec<String> = store
            .all_symbols()
            .unwrap()
            .into_iter()
            .map(|symbol| format!("{}|{}", symbol.path, symbol.qualified_name))
            .collect();
        let ranges = count(&db, "SELECT COUNT(*) FROM file_rows");
        (all_generations(&db), order, ranges)
    };
    let digests_written = payload_digests(&db);
    assert!(!digests_written.is_empty());
    assert_eq!(expected.len(), 5, "the history holds five generations");
    assert!(
        expected.iter().all(|(_, rows)| !rows.is_empty()),
        "every generation must read rows, or comparing them proves nothing"
    );

    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute_batch(v27_shape::REDUCE_V28_TO_V27).unwrap();
        conn.execute_batch("PRAGMA user_version = 27;").unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
                    AND name IN ('generation_nodes', 'generation_literals',
                                 'generation_file_rows', 'generation_file_digests',
                                 'nodes_fts_map')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 5, "the reduction must leave a v27 shape to migrate");
    }

    let store = Store::open(&db).expect("a v27 store must migrate");
    let order: Vec<String> = store
        .all_symbols()
        .unwrap()
        .into_iter()
        .map(|symbol| format!("{}|{}", symbol.path, symbol.qualified_name))
        .collect();
    drop(store);

    assert_eq!(
        count(&db, "PRAGMA user_version"),
        i64::from(CURRENT_SCHEMA_VERSION)
    );
    assert_eq!(
        payload_digests(&db),
        digests_written,
        "the digest the migration computes from the stored documents is the one \
         the writer stamped, so a --full after the migration reuses every payload"
    );
    let migrated = all_generations(&db);
    assert_eq!(
        migrated, expected,
        "a migrated store must read, generation by generation, what it read before"
    );
    assert_eq!(
        order, expected_order,
        "symbols must come back in the order v27 wrote them (row_id preserves it)"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM file_rows"),
        ranges,
        "consecutive generations holding one payload become one range, not one row each"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
        0,
        "no migrated row names a payload or a path that is not there"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM pragma_integrity_check WHERE integrity_check <> 'ok'"
        ),
        0
    );
    // And the migration is complete rather than a copy of the readable part: no
    // v27 table is left behind.
    for gone in ["nodes_fts_map", "generation_nodes", "generation_literals"] {
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = '{gone}' AND type = 'table'"
                )
            ),
            0,
            "{gone} must be a view after the migration"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// **Migration atomicity.** A migration that fails part-way leaves the v27 store
/// exactly as it found it: same stamp, same flat tables, none of v28's.
///
/// Failed here by planting a table of the wrong shape under one of v28's names,
/// so the rung's `CREATE TABLE IF NOT EXISTS` is silently satisfied and its first
/// `INSERT` into it raises -- after the rung has already created other tables.
/// A SIGKILL mid-migration is the same event to SQLite (the writer's transaction
/// never commits and the WAL discards it); that case is exercised with a real
/// process kill in the lane report. This is the deterministic half.
#[test]
fn a_migration_that_fails_part_way_leaves_the_v27_store_untouched() {
    let dir = tmp_dir("migration-atomic");
    let db = dir.join("devmap.sqlite");
    {
        let store = Store::open(&db).unwrap();
        history(&store);
    }
    let before_rows = all_generations(&db);
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute_batch(v27_shape::REDUCE_V28_TO_V27).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 27;
             CREATE TABLE file_rows (not_the_schema TEXT);",
        )
        .unwrap();
    }
    let flat_before = count(&db, "SELECT COUNT(*) FROM generation_nodes");
    assert!(flat_before > 0);

    assert!(
        Store::open(&db).is_err(),
        "a migration that cannot complete must refuse the store"
    );
    assert_eq!(
        count(&db, "PRAGMA user_version"),
        27,
        "the stamp must not advance past work that did not happen"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM generation_nodes"),
        flat_before,
        "the v27 tables must be whole"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN
                ('payload_nodes', 'payload_literals', 'digest_rows')"
        ),
        0,
        "none of the tables the failed rung created before it failed may survive"
    );
    // Retry after the obstruction is removed completes, and reads what it did.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("DROP TABLE file_rows;").unwrap();
    }
    drop(Store::open(&db).expect("the retry completes"));
    assert_eq!(all_generations(&db), before_rows);
    let _ = fs::remove_dir_all(&dir);
}

/// **The floor.** A v28 store tells every reader older than 28 to refuse it.
///
/// `reader_floor.rs` proves the refusal for every stamped version and floor; this
/// pins what v28 records, because the views keep every v27 column and a floor
/// left at 25 would have let a v27 reader through to join the new full-text
/// rowids by the old arithmetic.
#[test]
fn a_v28_store_records_a_reader_floor_of_28() {
    let dir = tmp_dir("floor");
    let db = dir.join("devmap.sqlite");
    drop(Store::open(&db).unwrap());
    assert_eq!(CURRENT_SCHEMA_VERSION, 28);
    assert_eq!(MIN_READER_SCHEMA_VERSION, 28);
    assert_eq!(
        count(&db, "SELECT min_reader_schema FROM reader_compat"),
        28,
        "the writer must record a floor that excludes a v27 reader"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A new store is whole: v28's rung drops the six flat tables the earlier
/// batches made, and the pages they held must not be left on the freelist, or a
/// file with no content reads as one worth vacuuming.
#[test]
fn a_new_store_has_no_free_pages() {
    let dir = tmp_dir("fresh-freelist");
    let db = dir.join("devmap.sqlite");
    drop(Store::open(&db).unwrap());
    assert_eq!(count(&db, "PRAGMA freelist_count"), 0);
    let _ = fs::remove_dir_all(&dir);
}

/// A file reverted to content it held earlier reuses that payload and its rows:
/// nothing is stored twice, the generation reads what the earlier one read, and
/// the file lists last because its membership is newer -- as v27 listed it.
#[test]
fn a_revert_reuses_the_payload_and_its_rows() {
    let dir = tmp_dir("revert");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let a = tree(6, |_| 0);
    let b = tree(6, |index| usize::from(index == 3));
    let g1 = commit(&store, &a, &[], &[]);
    let g2 = commit(&store, &b, &["pkg/mod_3.py"], &[]);
    let nodes_after_edit = count(&db, "SELECT COUNT(*) FROM payload_nodes");
    let payloads_after_edit = count(&db, "SELECT COUNT(*) FROM file_payloads");
    let g3 = commit(&store, &a, &["pkg/mod_3.py"], &[]);
    assert_eq!((g1, g2, g3), (1, 2, 3));

    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM file_payloads"),
        payloads_after_edit,
        "the reverted bytes are a payload the store already holds"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM payload_nodes"),
        nodes_after_edit,
        "and the symbols it owns are not written a second time"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM nodes_fts"),
        nodes_after_edit,
        "nor are its full-text entries"
    );
    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        read_generation(&conn, g3),
        read_generation(&conn, g1),
        "the reverted generation reads what the original read"
    );
    assert_ne!(read_generation(&conn, g2), read_generation(&conn, g1));
    // Three ranges for the one file: the original's, the edit's, the revert's.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM file_rows r JOIN paths p ON p.id = r.file_id
              WHERE p.path = 'pkg/mod_3.py'"
        ),
        3
    );
    let names: Vec<String> = store
        .all_symbols()
        .unwrap()
        .into_iter()
        .map(|symbol| symbol.path)
        .collect();
    assert_eq!(
        names.last().map(String::as_str),
        Some("pkg/mod_3.py"),
        "a reverted file lists after the files this build carried"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Two files with identical content are two payloads: a payload is keyed by its
/// file, so each reads its own path -- and an edit to one leaves the other's
/// rows alone.
#[test]
fn two_files_with_identical_bytes_keep_their_own_rows() {
    let dir = tmp_dir("twins");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let shared = "SAME = \"shared-literal\"\n\n\ndef shared_fn():\n    return 1\n".to_string();
    let files: Files = vec![
        ("a/one.py".into(), shared.clone()),
        ("b/two.py".into(), shared.clone()),
    ];
    let g1 = commit(&store, &files, &[], &[]);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM file_payloads"), 2);
    let conn = Connection::open(&db).unwrap();
    let rows = read_generation(&conn, g1);
    for path in ["a/one.py", "b/two.py"] {
        assert!(
            rows.iter()
                .any(|row| row.starts_with(&format!("node|{path}|shared_fn"))),
            "{path} reads its own symbol"
        );
        assert!(
            rows.iter()
                .any(|row| row.starts_with(&format!("literal|{path}|"))
                    && row.contains("shared-literal")),
            "{path} reads its own literal"
        );
    }
    let edited: Files = vec![
        (
            "a/one.py".into(),
            format!("{shared}\n\ndef only_here():\n    return 2\n"),
        ),
        ("b/two.py".into(), shared),
    ];
    let g2 = commit(&store, &edited, &["a/one.py"], &[]);
    let rows = read_generation(&conn, g2);
    assert!(rows
        .iter()
        .any(|row| row.starts_with("node|a/one.py|only_here")));
    assert!(!rows
        .iter()
        .any(|row| row.starts_with("node|b/two.py|only_here")));
    assert!(
        rows.iter()
            .any(|row| row.starts_with("node|b/two.py|shared_fn")),
        "the untouched twin keeps its rows"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Retention collects a payload exactly when no retained range names it, and
/// takes everything derived from it: its symbols, its literals, its full-text
/// entries. Content seen before collection reuses the payload; content seen
/// after it is written afresh and reads correctly.
#[test]
fn retention_collects_a_payload_with_its_rows_and_only_then() {
    let dir = tmp_dir("gc");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let revision = |n: usize| tree(5, move |index| if index == 2 { n } else { 0 });
    let mut generations = Vec::new();
    for n in 0..4 {
        generations.push(commit(&store, &revision(n), &["pkg/mod_2.py"], &[]));
        store.prune_generations_except_latest(2).unwrap();
    }
    // Two retained generations of one edited file: its two newest payloads, and
    // the four unedited files' payloads.
    assert_eq!(count(&db, "SELECT COUNT(*) FROM generations"), 2);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM file_payloads"), 4 + 2);
    for orphaned in [
        "SELECT COUNT(*) FROM payload_nodes WHERE payload_id NOT IN (SELECT payload_id FROM file_payloads)",
        "SELECT COUNT(*) FROM payload_literals WHERE payload_id NOT IN (SELECT payload_id FROM file_payloads)",
        "SELECT COUNT(*) FROM file_payloads WHERE payload_id NOT IN (SELECT payload_id FROM file_rows)",
        "SELECT COUNT(*) FROM nodes_fts WHERE (rowid >> 24) NOT IN (SELECT payload_id FROM file_payloads)",
    ] {
        assert_eq!(count(&db, orphaned), 0, "{orphaned}");
    }
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM nodes_fts"),
        count(&db, "SELECT COUNT(*) FROM payload_nodes"),
        "the index holds the retained payloads' symbols and no others"
    );

    // Revision 0 was collected two prunes ago: committing it again is a new
    // payload, and the generation reads it in full.
    let again = commit(&store, &revision(0), &["pkg/mod_2.py"], &[]);
    let conn = Connection::open(&db).unwrap();
    let rows = read_generation(&conn, again);
    assert!(rows
        .iter()
        .any(|row| row.starts_with("node|pkg/mod_2.py|leaf_2")));
    assert!(rows
        .iter()
        .any(|row| row.starts_with("literal|pkg/mod_2.py|")));
    let (path, body) = module(2, 0);
    assert_eq!(
        rows.iter()
            .filter(|row| row.starts_with("node|pkg/mod_2.py|"))
            .count(),
        extract_file(&path, &body).symbols.len(),
        "every symbol the extractor found in the module"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A reader running against a writer that commits and prunes continuously sees
/// one generation at a time: every answer is the symbol set of some committed
/// generation, never a mixture of two, and no read fails.
#[test]
fn a_reader_against_a_writing_and_pruning_writer_sees_one_generation() {
    let dir = tmp_dir("concurrent");
    let db = dir.join("devmap.sqlite");
    let writer = Store::open(&db).unwrap();
    let revision = |n: usize| tree(6, move |index| if index == 4 { n } else { 0 });
    commit(&writer, &revision(0), &[], &[]);

    // What each revision reads: the whole symbol set, as `path|qualified_name`.
    let expected: Arc<BTreeSet<Vec<String>>> = Arc::new(
        (0..12)
            .map(|n| {
                let mut names: Vec<String> = revision(n)
                    .iter()
                    .flat_map(|(path, body)| {
                        extract_file(path, body)
                            .symbols
                            .into_iter()
                            .map(|symbol| format!("{path}|{}", symbol.qualified_name))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                names.sort();
                names
            })
            .collect(),
    );

    let stop = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicU64::new(0));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (db, stop, expected, started) =
                (db.clone(), stop.clone(), expected.clone(), started.clone());
            std::thread::spawn(move || {
                let reader = Store::open(&db).unwrap();
                let mut reads = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let mut names: Vec<String> = reader
                        .all_symbols()
                        .expect("a read during a write must not fail")
                        .into_iter()
                        .map(|symbol| format!("{}|{}", symbol.path, symbol.qualified_name))
                        .collect();
                    names.sort();
                    assert!(
                        expected.contains(&names),
                        "a reader saw a symbol set no generation held ({} symbols)",
                        names.len()
                    );
                    let page = reader
                        .search_page("leaf_4", 50)
                        .expect("a search during a write must not fail")
                        .expect("a generation exists");
                    assert!(page.total >= 1, "leaf_4 is in every generation");
                    reads += 1;
                    if reads == 1 {
                        started.fetch_add(1, Ordering::SeqCst);
                    }
                }
                reads
            })
        })
        .collect();

    // Every reader has answered once against the first generation before the
    // writer moves, and keeps answering while it commits and prunes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while started.load(Ordering::SeqCst) < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for n in 1..12 {
        commit(&writer, &revision(n), &["pkg/mod_4.py"], &[]);
        writer.prune_generations_except_latest(2).unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    let total: u32 = readers
        .into_iter()
        .map(|reader| reader.join().unwrap())
        .sum();
    assert!(
        total > 0,
        "the readers must have read while the writer wrote"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A snapshot held open across the writer's commits and the retention prune
/// keeps reading the generation it began on: the prune deletes the payload rows
/// and full-text entries that generation named, and the reader, which SQLite
/// serves from the pages as they were when it began, still finds every one.
///
/// The other half of "GC racing a reader": the test above reads between writes,
/// this one reads *across* them.
#[test]
fn a_snapshot_held_across_commits_and_prunes_keeps_reading_its_generation() {
    let dir = tmp_dir("held-snapshot");
    let db = dir.join("devmap.sqlite");
    let writer = Store::open(&db).unwrap();
    let revision = |n: usize| tree(6, move |index| if index == 4 { n } else { 0 });
    let first = commit(&writer, &revision(0), &[], &[]);

    let mut reader = Connection::open(&db).unwrap();
    let snapshot = reader.transaction().unwrap();
    let before = {
        let mut stmt = snapshot
            .prepare(
                "SELECT 'node|' || n.qualified_name || '|' || n.span_start
                   FROM generation_nodes n WHERE n.generation_id = ?1
                 UNION ALL
                 SELECT 'fts|' || f.qualified_name
                   FROM nodes_fts_map m JOIN nodes_fts f ON f.rowid = m.rowid_ref
                  WHERE m.generation_id = ?1
                 UNION ALL
                 SELECT 'literal|' || l.value FROM generation_literals l
                  WHERE l.generation_id = ?1
                 ORDER BY 1",
            )
            .unwrap();
        stmt.query_map([first], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    assert!(
        before.len() > 20,
        "the snapshot must hold something to lose"
    );

    // Enough generations that revision 0's payload for mod_4 is pruned and
    // collected, and its rows with it.
    for n in 1..6 {
        commit(&writer, &revision(n), &["pkg/mod_4.py"], &[]);
        writer.prune_generations_except_latest(2).unwrap();
    }
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM generations WHERE id = {first}")
        ),
        0,
        "the generation the reader began on is gone for every new connection"
    );

    let after = {
        let mut stmt = snapshot
            .prepare(
                "SELECT 'node|' || n.qualified_name || '|' || n.span_start
                   FROM generation_nodes n WHERE n.generation_id = ?1
                 UNION ALL
                 SELECT 'fts|' || f.qualified_name
                   FROM nodes_fts_map m JOIN nodes_fts f ON f.rowid = m.rowid_ref
                  WHERE m.generation_id = ?1
                 UNION ALL
                 SELECT 'literal|' || l.value FROM generation_literals l
                  WHERE l.generation_id = ?1
                 ORDER BY 1",
            )
            .unwrap();
        stmt.query_map([first], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        after, before,
        "a held snapshot reads the generation it began on, whatever was pruned since"
    );
    drop(snapshot);
    let _ = fs::remove_dir_all(&dir);
}

/// A file with 50,000 literal sites: the literals are written once, found by
/// value, and never rewritten by an edit elsewhere.
///
/// The extractor stops at `MAX_LITERALS_PER_FILE` (4,096) per file, so a real
/// source file cannot reach 50,000; the extraction is built by hand to put the
/// store itself, which has no such cap, under that many rows for one payload.
#[test]
fn a_file_of_fifty_thousand_literals_is_written_once() {
    let dir = tmp_dir("literals");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let mut table = extract_file("data/table.py", "TABLE = []\n");
    table.literals = (0..50_000)
        .map(|index| devmap_extract::ExtractedLiteral {
            value: format!("v{index}"),
            start_byte: 9 + index,
            line: 1,
            enclosing_qualified_name: String::new(),
            enclosing_name: String::new(),
        })
        .collect();
    let mut extractions = vec![table];
    extractions.extend(tree(3, |_| 0).iter().map(|(p, b)| extract_file(p, b)));
    commit_extractions(&store, &extractions, &[], &[]);
    let written = count(&db, "SELECT COUNT(*) FROM payload_literals");
    assert!(written >= 50_000, "every literal site is stored: {written}");

    let page = store
        .search_literals("v49999", true, 10)
        .unwrap()
        .expect("a generation exists");
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0].file_path, "data/table.py");

    let (path, body) = module(0, 9);
    extractions[1] = extract_file(&path, &body);
    commit_extractions(&store, &extractions, &["pkg/mod_0.py"], &[]);
    let grew = count(&db, "SELECT COUNT(*) FROM payload_literals") - written;
    assert!(
        grew < 10,
        "an edit to another file wrote {grew} literal rows; the 50,000 are the table's payload's"
    );
    let page = store
        .search_literals("v49999", true, 10)
        .unwrap()
        .expect("a generation exists");
    assert_eq!(page.total, 1, "the table still reads in the new generation");
    // Both retained generations read all 50,000 through the view.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM generation_literals l JOIN paths p ON p.id = l.file_id
              WHERE p.path = 'data/table.py' AND l.generation_id = 2"
        ),
        50_000
    );
    let _ = fs::remove_dir_all(&dir);
}

/// An empty file and a file no grammar reads are members like any other: a
/// payload, a range, no symbols, and no failure writing them twice.
#[test]
fn an_empty_file_and_an_unsupported_file_are_members_without_symbols() {
    let dir = tmp_dir("degenerate");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let mut files: Files = vec![
        ("empty.py".into(), String::new()),
        ("blob.bin".into(), "\u{0}\u{1}\u{2}binary\u{0}\n".into()),
    ];
    files.extend(tree(2, |_| 0));
    let g1 = commit(&store, &files, &[], &[]);
    let conn = Connection::open(&db).unwrap();
    for (path, body) in files.iter().take(2) {
        let path = path.as_str();
        let symbols = extract_file(path, body).symbols.len() as i64;
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM generation_files f JOIN paths p ON p.id = f.file_id
                      WHERE f.generation_id = {g1} AND p.path = '{path}'"
                )
            ),
            1,
            "{path} is a member of the generation"
        );
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM generation_nodes n JOIN paths p ON p.id = n.file_id
                      WHERE n.generation_id = {g1} AND p.path = '{path}'"
                )
            ),
            symbols,
            "{path} holds the symbols its extraction found (none from a grammar-less or empty file beyond what the extractor itself reports)"
        );
    }
    // A full rewrite over the same content writes no new payload and no range.
    let payloads = count(&db, "SELECT COUNT(*) FROM file_payloads");
    let ranges = count(&db, "SELECT COUNT(*) FROM file_rows");
    let g2 = commit(&store, &files, &[], &[]);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM file_payloads"), payloads);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM file_rows"), ranges);
    assert_eq!(read_generation(&conn, g2), read_generation(&conn, g1));
    let _ = fs::remove_dir_all(&dir);
}

/// A full rewrite over a tree that has lost a file closes that file's membership
/// even though nothing declared it deleted: the generation is exactly the files
/// the write was handed, and a range that outlived its file would keep
/// reporting symbols of a file the tree no longer has.
#[test]
fn a_full_rewrite_closes_the_membership_of_a_file_that_is_gone() {
    let dir = tmp_dir("full-rewrite");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(5, |_| 0);
    let g1 = commit(&store, &files, &[], &[]);
    let without: Files = files
        .iter()
        .filter(|(path, _)| path != "pkg/mod_3.py")
        .cloned()
        .collect();
    let g2 = commit(&store, &without, &[], &[]);
    let conn = Connection::open(&db).unwrap();
    assert!(read_generation(&conn, g1)
        .iter()
        .any(|row| row.starts_with("node|pkg/mod_3.py|")));
    assert!(
        !read_generation(&conn, g2)
            .iter()
            .any(|row| row.contains("pkg/mod_3.py")),
        "the file is not in the generation, in any relation"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT valid_to FROM file_rows r JOIN paths p ON p.id = r.file_id
                  WHERE p.path = 'pkg/mod_3.py' AND r.valid_to = {g2}"
            )
        ),
        i64::from(g2)
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
        0,
        "no row names a payload or a path that is not there"
    );
    // The old generation still reads the file until retention takes it, and then
    // the payload and everything derived from it go together.
    store.prune_generations_except_latest(1).unwrap();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM file_payloads fp JOIN paths p ON p.id = fp.file_id
              WHERE p.path = 'pkg/mod_3.py'"
        ),
        0
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A secret file's literal values are withheld in the payload-scoped table too:
/// there is one copy of a literal now, so there is one place to scrub.
#[test]
fn a_secret_files_literals_are_withheld_in_the_payload_table() {
    let dir = tmp_dir("secret");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let files: Files = vec![
        ("pkg/ok.py".into(), "NAME = \"public-value\"\n".into()),
        (
            ".env".into(),
            "API_KEY=\"sk-live-0123456789abcdef\"\n".into(),
        ),
    ];
    commit(&store, &files, &[], &[]);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM payload_literals WHERE value LIKE '%sk-live-0123456789abcdef%'"
        ),
        0,
        "no payload row holds a secret file's value"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Fable ruling 1: a payload's rows must be the extraction's rows.
//
// The identity a payload is stored under is the file, its content, and the
// extractor's version strings. A build of a *different binary* at the same
// version strings (the open extract-98 policy: lane binaries change extractor
// output without a bump) produces other rows for the same key. v28 first
// reused the stored row whenever the symbol count matched, so a same-count
// change of kind, name, span or literal stayed stale in every relation the
// payload owns, and `build --full` -- which re-parses and writes every file
// through the same entry point -- did not clear it.
// ---------------------------------------------------------------------------

use devmap_extract::cache::CacheKey;
use devmap_extract::{Extraction, SymbolKind};

/// What a generation reads for one file, by relation: symbols as
/// `name|kind|start|end`, literals as `start|value`, and the full-text names
/// that `search` answers from.
#[derive(Debug, PartialEq, Eq)]
struct FileRows {
    nodes: Vec<String>,
    literals: Vec<String>,
    fts: Vec<String>,
}

fn file_rows(db: &Path, generation: u32, path: &str) -> FileRows {
    let conn = Connection::open(db).unwrap();
    let collect = |sql: &str| -> Vec<String> {
        let mut rows: Vec<String> = conn
            .prepare(sql)
            .unwrap()
            .query_map(rusqlite::params![generation, path], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.sort();
        rows
    };
    FileRows {
        nodes: collect(
            "SELECT n.name || '|' || n.kind || '|' || n.span_start || '|' || n.span_end
               FROM generation_nodes n JOIN paths p ON p.id = n.file_id
              WHERE n.generation_id = ?1 AND p.path = ?2",
        ),
        literals: collect(
            "SELECT l.span_start || '|' || l.value
               FROM generation_literals l JOIN paths p ON p.id = l.file_id
              WHERE l.generation_id = ?1 AND p.path = ?2",
        ),
        fts: collect(
            "SELECT f.name || '|' || f.qualified_name
               FROM nodes_fts_map m JOIN nodes_fts f ON f.rowid = m.rowid_ref
              WHERE m.generation_id = ?1 AND f.path = ?2",
        ),
    }
}

fn payloads_of(db: &Path, path: &str) -> i64 {
    count(
        db,
        &format!(
            "SELECT COUNT(*) FROM file_payloads fp JOIN paths p ON p.id = fp.file_id
              WHERE p.path = '{path}'"
        ),
    )
}

fn ranges_of(db: &Path, path: &str) -> i64 {
    count(
        db,
        &format!(
            "SELECT COUNT(*) FROM file_rows r JOIN paths p ON p.id = r.file_id
              WHERE p.path = '{path}'"
        ),
    )
}

/// The same bytes, extracted by "another binary": one row differs and the count
/// of symbols and literals does not.
fn another_binary(extraction: &Extraction, change: &str) -> Extraction {
    let mut other = extraction.clone();
    match change {
        "kind" => {
            assert_ne!(other.symbols[0].kind, SymbolKind::Class);
            other.symbols[0].kind = SymbolKind::Class;
        }
        "name" => {
            other.symbols[0].name.push_str("_renamed");
            other.symbols[0].qualified_name.push_str("_renamed");
        }
        "span" => other.symbols[0].span.end_byte += 1,
        "literal" => other.literals[0].value = "label-from-another-binary".into(),
        other_change => panic!("unknown change {other_change}"),
    }
    other
}

/// **The blocking bug.** Same file, same content, same extractor identity,
/// same symbol count, other rows: the rows the store reads must be the rows of
/// the extraction in hand, for a differential write and for `--full` (no
/// affected paths), in every relation; the generation that was built before
/// must still read what it read; and the older rows are a payload of their own
/// rather than a rewritten one.
fn same_count_change_is_stored_as_the_extraction_in_hand_says(change: &str, full: bool) {
    let label = format!("{change} (full: {full})");
    let dir = tmp_dir("same-count");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let first = extract_file(&path, &body);
    let second = another_binary(&first, change);
    assert_eq!(
        CacheKey::for_extraction(&first),
        CacheKey::for_extraction(&second),
        "{label}: the test must exercise the identity the payload is keyed on"
    );
    assert_eq!(first.symbols.len(), second.symbols.len());
    assert_eq!(first.literals.len(), second.literals.len());

    let g1 = commit_extractions(&store, std::slice::from_ref(&first), &[], &[]);
    let before = file_rows(&db, g1, &path);
    let affected: &[&str] = if full { &[] } else { &[path.as_str()] };
    let g2 = commit_extractions(&store, std::slice::from_ref(&second), affected, &[]);

    let after = file_rows(&db, g2, &path);
    assert_ne!(
        after, before,
        "{label}: the new generation read the old rows"
    );
    // What the second extraction would have written to a store of its own.
    let fresh_dir = tmp_dir("same-count-fresh");
    let fresh_db = fresh_dir.join("devmap.sqlite");
    let fresh = Store::open(&fresh_db).unwrap();
    let fg = commit_extractions(&fresh, std::slice::from_ref(&second), &[], &[]);
    assert_eq!(
        after,
        file_rows(&fresh_db, fg, &path),
        "{label}: rows differ from a store built from the extraction alone"
    );
    assert_eq!(
        file_rows(&db, g1, &path),
        before,
        "{label}: the retained generation's rows were changed underneath it"
    );
    assert_eq!(payloads_of(&db, &path), 2, "{label}: payloads");
    assert_eq!(ranges_of(&db, &path), 2, "{label}: ranges");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&fresh_dir);
}

macro_rules! same_count_change {
    ($($name:ident: $change:literal, $full:literal;)*) => {
        $(
            #[test]
            fn $name() {
                same_count_change_is_stored_as_the_extraction_in_hand_says($change, $full);
            }
        )*
    };
}

same_count_change! {
    a_changed_kind_is_stored_by_a_differential_write: "kind", false;
    a_changed_kind_is_stored_by_a_full_rebuild: "kind", true;
    a_changed_name_is_stored_by_a_differential_write: "name", false;
    a_changed_name_is_stored_by_a_full_rebuild: "name", true;
    a_changed_span_is_stored_by_a_differential_write: "span", false;
    a_changed_span_is_stored_by_a_full_rebuild: "span", true;
    a_changed_literal_is_stored_by_a_differential_write: "literal", false;
    a_changed_literal_is_stored_by_a_full_rebuild: "literal", true;
}

/// The reuse the fix must keep: the same extraction again, differentially or
/// with `--full`, writes no payload and no range.
#[test]
fn the_same_extraction_again_is_the_same_payload() {
    for full in [false, true] {
        let dir = tmp_dir("same-again");
        let db = dir.join("devmap.sqlite");
        let store = Store::open(&db).unwrap();
        let files = tree(4, |_| 0);
        commit(&store, &files, &[], &[]);
        let payloads = count(&db, "SELECT COUNT(*) FROM file_payloads");
        let nodes = count(&db, "SELECT COUNT(*) FROM payload_nodes");
        let ranges = count(&db, "SELECT COUNT(*) FROM file_rows");
        let affected: &[&str] = if full { &[] } else { &["pkg/mod_2.py"] };
        commit(&store, &files, affected, &[]);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM file_payloads"),
            payloads,
            "full: {full}"
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM payload_nodes"),
            nodes,
            "full: {full}"
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM file_rows"),
            ranges,
            "full: {full}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

/// A reused payload whose rows are not all there is repaired to the
/// extraction's, not trusted: a store carrying a payload without rows (a v27
/// store's payload that no generation named, migrated with nothing to copy)
/// must not make a file vanish from the generation that reuses it.
#[test]
fn a_reused_payload_missing_its_rows_gets_them_back() {
    let dir = tmp_dir("repair");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let first = extract_file(&path, &body);
    let g1 = commit_extractions(&store, std::slice::from_ref(&first), &[], &[]);
    let before = file_rows(&db, g1, &path);
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "DELETE FROM nodes_fts; DELETE FROM payload_nodes; DELETE FROM payload_literals;",
        )
        .unwrap();
    }
    assert!(file_rows(&db, g1, &path).nodes.is_empty());
    let g2 = commit_extractions(&store, std::slice::from_ref(&first), &[], &[]);
    assert_eq!(file_rows(&db, g2, &path), before);
    let _ = fs::remove_dir_all(&dir);
}

/// The budget refusal that a later build recovers from. The refusal and the
/// recovery are different extractions of the same key: each is its own payload,
/// each generation reads the rows of its own, and a refusal arriving over a
/// recovered payload does not delete the rows of the generation that holds it.
#[test]
fn a_refusal_and_its_recovery_are_two_payloads_with_their_own_rows() {
    use devmap_extract::extract_file_with_budget;
    use std::time::Duration;
    let dir = tmp_dir("refusal");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let refused = extract_file_with_budget(&path, &body, Duration::ZERO);
    assert!(refused.is_budget_refusal());
    let recovered = extract_file_with_budget(&path, &body, Duration::from_secs(60));
    assert!(!recovered.is_parse_failure());
    assert!(!recovered.symbols.is_empty());

    let g1 = commit_extractions(&store, std::slice::from_ref(&refused), &[], &[]);
    let refused_rows = file_rows(&db, g1, &path);
    assert_eq!(refused_rows.nodes.len(), refused.symbols.len());
    let g2 = commit_extractions(
        &store,
        std::slice::from_ref(&recovered),
        &[path.as_str()],
        &[],
    );
    let recovered_rows = file_rows(&db, g2, &path);
    assert_eq!(recovered_rows.nodes.len(), recovered.symbols.len());
    assert_eq!(recovered_rows.fts.len(), recovered.symbols.len());
    assert_eq!(
        file_rows(&db, g1, &path),
        refused_rows,
        "the generation built from the refusal still reads the refusal's rows"
    );
    assert_eq!(payloads_of(&db, &path), 2);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE parse_outcome_json LIKE '%budget exceeded%'"
        ),
        1,
        "the refusal's own payload keeps its outcome"
    );

    // The refusal again: it is the payload the store already holds, and the
    // recovered generation's rows are not touched.
    let g3 = commit_extractions(
        &store,
        std::slice::from_ref(&refused),
        &[path.as_str()],
        &[],
    );
    assert_eq!(file_rows(&db, g3, &path), refused_rows);
    assert_eq!(file_rows(&db, g2, &path), recovered_rows);
    assert_eq!(payloads_of(&db, &path), 2);
    let _ = fs::remove_dir_all(&dir);
}

/// The refusal and its recovery, end to end through retention: two payloads
/// that both exist, each generation reading its own payload's rows, and the
/// older generation's pruning collecting the refusal payload with every row
/// derived from it while the recovery's stay.
///
/// `the_older_output_of_a_key_is_collected_with_its_rows_by_retention` pins the
/// same collection for two outputs of a parsed file; this is the shape the
/// digest exists for, a budget refusal over the same key as a parse.
#[test]
fn a_refusal_and_its_recovery_are_collected_independently_by_retention() {
    use devmap_extract::extract_file_with_budget;
    use std::time::Duration;
    let dir = tmp_dir("refusal-collect");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let refused = extract_file_with_budget(&path, &body, Duration::ZERO);
    assert!(refused.is_budget_refusal());
    let recovered = extract_file_with_budget(&path, &body, Duration::from_secs(60));
    assert!(!recovered.is_parse_failure());
    assert!(!recovered.symbols.is_empty());

    let g1 = commit_extractions(&store, std::slice::from_ref(&refused), &[], &[]);
    let g2 = commit_extractions(
        &store,
        std::slice::from_ref(&recovered),
        &[path.as_str()],
        &[],
    );
    let refused_rows = file_rows(&db, g1, &path);
    let recovered_rows = file_rows(&db, g2, &path);
    assert_ne!(
        refused_rows, recovered_rows,
        "the two generations must read different rows for this test to mean anything"
    );
    assert_eq!(refused_rows.nodes.len(), refused.symbols.len());
    assert_eq!(recovered_rows.nodes.len(), recovered.symbols.len());

    // Both payloads exist, and they are two rows, not one rewritten.
    let payload_where = |predicate: &str| -> i64 {
        count(
            &db,
            &format!(
                "SELECT payload_id FROM file_payloads
                  WHERE parse_outcome_json {predicate} '%budget exceeded%'"
            ),
        )
    };
    let refusal_payload = payload_where("LIKE");
    let recovery_payload = payload_where("NOT LIKE");
    assert_ne!(refusal_payload, recovery_payload);
    assert_eq!(payloads_of(&db, &path), 2);
    let rows_of = |relation: &str, payload: i64| -> i64 {
        count(
            &db,
            &format!("SELECT COUNT(*) FROM {relation} WHERE payload_id = {payload}"),
        )
    };
    assert_eq!(
        rows_of("payload_nodes", refusal_payload),
        refused.symbols.len() as i64
    );
    assert_eq!(
        rows_of("payload_nodes", recovery_payload),
        recovered.symbols.len() as i64
    );

    // Each generation reads its own payload's rows.
    assert_eq!(file_rows(&db, g1, &path), refused_rows);
    assert_eq!(file_rows(&db, g2, &path), recovered_rows);

    // Retention drops the older generation: nothing retained names the refusal
    // payload any more, so it goes with the rows derived from it, and the
    // recovery's are untouched.
    store.prune_generations_except_latest(1).unwrap();
    assert_eq!(payloads_of(&db, &path), 1);
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM file_payloads WHERE payload_id = {refusal_payload}")
        ),
        0,
        "the refusal's payload is collected"
    );
    assert_eq!(rows_of("payload_nodes", refusal_payload), 0);
    assert_eq!(rows_of("payload_literals", refusal_payload), 0);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM payload_nodes"),
        recovered.symbols.len() as i64,
        "only the recovery's rows remain"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM nodes_fts"),
        recovered.symbols.len() as i64,
        "and only its full-text rows"
    );
    assert_eq!(
        rows_of("payload_nodes", recovery_payload),
        recovered.symbols.len() as i64
    );
    assert_eq!(file_rows(&db, g2, &path), recovered_rows);
    let _ = fs::remove_dir_all(&dir);
}

/// The extraction pass falls back to a retained generation's payload by content
/// identity. With two payloads under one identity it must hand back the newest
/// one, not whichever row SQLite reaches first.
#[test]
fn the_cache_fallback_reads_the_newest_payload_of_an_identity() {
    let dir = tmp_dir("fallback");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let first = extract_file(&path, &body);
    let second = another_binary(&first, "kind");
    let key = CacheKey::for_extraction(&first);
    commit_extractions(&store, std::slice::from_ref(&first), &[], &[]);
    commit_extractions(&store, std::slice::from_ref(&second), &[path.as_str()], &[]);
    let served = store
        .try_get_cached_extraction(&key)
        .unwrap()
        .expect("a retained generation holds this identity");
    assert_eq!(served.symbols[0].kind, SymbolKind::Class);
    let _ = fs::remove_dir_all(&dir);
}

/// A build that suffered a wall-clock refusal must not turn the next build's
/// cache fallback into a refusal for bytes that parse: the newest generation
/// holds the refusal, and the fallback serves the payload that does not.
#[test]
fn the_cache_fallback_never_serves_a_failed_parse() {
    use devmap_extract::extract_file_with_budget;
    use std::time::Duration;
    let dir = tmp_dir("fallback-refusal");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let recovered = extract_file_with_budget(&path, &body, Duration::from_secs(60));
    let refused = extract_file_with_budget(&path, &body, Duration::ZERO);
    assert!(refused.is_budget_refusal());
    let key = CacheKey::for_extraction(&recovered);
    commit_extractions(&store, std::slice::from_ref(&recovered), &[], &[]);
    commit_extractions(
        &store,
        std::slice::from_ref(&refused),
        &[path.as_str()],
        &[],
    );
    let served = store
        .try_get_cached_extraction(&key)
        .unwrap()
        .expect("generation 1 holds the parse");
    assert!(!served.is_parse_failure());
    assert_eq!(served.symbols.len(), recovered.symbols.len());
    let _ = fs::remove_dir_all(&dir);
}

/// The older output of a same-key pair is a payload like any other: retention
/// collects it, with the rows derived from it, once no retained range names it,
/// and not before.
#[test]
fn the_older_output_of_a_key_is_collected_with_its_rows_by_retention() {
    let dir = tmp_dir("collect-older-output");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let (path, body) = module(0, 0);
    let first = extract_file(&path, &body);
    let second = another_binary(&first, "name");
    let g1 = commit_extractions(&store, std::slice::from_ref(&first), &[], &[]);
    let g2 = commit_extractions(&store, std::slice::from_ref(&second), &[path.as_str()], &[]);
    let rows_of_both = count(&db, "SELECT COUNT(*) FROM payload_nodes");
    assert_eq!(rows_of_both, 8, "two payloads of four symbols");
    assert_eq!(
        file_rows(&db, g1, &path).nodes.len(),
        4,
        "generation {g1} is retained"
    );

    store.prune_generations_except_latest(1).unwrap();
    assert_eq!(payloads_of(&db, &path), 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM payload_nodes"), 4);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM nodes_fts"), 4);
    assert_eq!(
        file_rows(&db, g2, &path).nodes.len(),
        4,
        "and the newer payload keeps every row it had"
    );
    assert!(file_rows(&db, g2, &path)
        .nodes
        .iter()
        .any(|node| node.starts_with("mod_0.py_renamed|")));
    let _ = fs::remove_dir_all(&dir);
}

/// A secret file's payload is rewritten in place by the scrub, so its stored
/// documents are not the ones its digest was taken from. The digest is a column
/// and not recomputed, so the next build of the same file is still the same
/// payload.
#[test]
fn a_secret_file_rebuilt_unchanged_is_the_same_payload() {
    for full in [false, true] {
        let dir = tmp_dir("secret-stable");
        let db = dir.join("devmap.sqlite");
        let store = Store::open(&db).unwrap();
        let files: Files = vec![
            ("pkg/ok.py".into(), "NAME = \"public-value\"\n".into()),
            (
                ".env".into(),
                "API_KEY=\"sk-live-0123456789abcdef\"\n".into(),
            ),
        ];
        commit(&store, &files, &[], &[]);
        let payloads = count(&db, "SELECT COUNT(*) FROM file_payloads");
        let affected: &[&str] = if full { &[] } else { &[".env"] };
        commit(&store, &files, affected, &[]);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM file_payloads"),
            payloads,
            "full: {full}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

/// A store the first v28 prototype wrote is stamped 28 and has no
/// `payload_digest`. It opens, is given the digests the writer would have
/// stamped, and its next full build reuses every payload.
#[test]
fn a_store_from_the_first_v28_prototype_is_healed_on_open() {
    let dir = tmp_dir("prototype-heal");
    let db = dir.join("devmap.sqlite");
    let digests = {
        let store = Store::open(&db).unwrap();
        history(&store);
        payload_digests(&db)
    };
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "DROP INDEX idx_file_payloads_digest_identity;
             ALTER TABLE file_payloads DROP COLUMN payload_digest;
             CREATE UNIQUE INDEX idx_file_payloads_identity
                 ON file_payloads(file_id, content_hash, language,
                                  COALESCE(grammar_version, ''),
                                  COALESCE(analyzer_version, ''));",
        )
        .unwrap();
    }
    let store = Store::open(&db).expect("a prototype store opens");
    assert_eq!(payload_digests(&db), digests);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'idx_file_payloads_identity'"
        ),
        0
    );
    let payloads = count(&db, "SELECT COUNT(*) FROM file_payloads");
    commit(&store, &tree(8, |index| usize::from(index == 2)), &[], &[]);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM file_payloads"),
        payloads,
        "a full build of the current tree reuses what the healed digests name"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A store whose `payload_digest` column exists but whose rows are not all
/// stamped: the shape a prototype writer left if it was killed between adding
/// the column and the backfill. (The ladder holds one transaction, so this
/// build's own migration cannot stop there; the heal must not rely on that.)
/// The open stamps every row and never proceeds with an empty digest, and the
/// next full build reuses every payload rather than writing a second copy.
#[test]
fn a_store_with_the_digest_column_but_unstamped_rows_is_stamped_on_open() {
    for unstamped in ["every row", "some rows"] {
        let dir = tmp_dir("interrupted-heal");
        let db = dir.join("devmap.sqlite");
        let digests = {
            let store = Store::open(&db).unwrap();
            history(&store);
            payload_digests(&db)
        };
        assert!(digests.iter().all(|(_, _, digest)| !digest.is_empty()));
        {
            let conn = Connection::open(&db).unwrap();
            let predicate = if unstamped == "every row" {
                "1 = 1"
            } else {
                "payload_id % 2 = 0"
            };
            conn.execute_batch(&format!(
                "DROP INDEX idx_file_payloads_digest_identity;
                 UPDATE file_payloads SET payload_digest = x'' WHERE {predicate};"
            ))
            .unwrap();
            assert!(
                count(
                    &db,
                    "SELECT COUNT(*) FROM file_payloads WHERE payload_digest = x''"
                ) > 0,
                "{unstamped}: the interrupted shape is set up"
            );
        }
        let store = Store::open(&db).expect("an interrupted-heal store opens");
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM file_payloads WHERE payload_digest = x''"
            ),
            0,
            "{unstamped}: no row is left unstamped"
        );
        assert_eq!(payload_digests(&db), digests, "{unstamped}");
        let payloads = count(&db, "SELECT COUNT(*) FROM file_payloads");
        commit(&store, &tree(8, |index| usize::from(index == 2)), &[], &[]);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM file_payloads"),
            payloads,
            "{unstamped}: a full build reuses every payload"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
