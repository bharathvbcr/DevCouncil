//! A damaged full-text index is reported as that, with the command that fixes it.
//!
//! The index lives in `nodes_fts` (an FTS5 table and its shadow tables) and
//! `nodes_fts_map`, apart from the symbol rows. Damage there surfaced as
//! SQLite's "database disk image is malformed" — a message about the whole
//! database, which invites a rebuild nobody needs — and `devmap status`, which
//! never read the index, reported the store healthy. `devmap repair --fts`
//! existed for exactly this state and nothing pointed at it.
//!
//! Two kinds of damage, each asserted against search *and* status:
//! - **unreadable**: the FTS5 structure record is overwritten, so every MATCH
//!   fails;
//! - **partially lost**: half of one generation's index rows are deleted, so
//!   search silently returns a subset and reports it as the whole answer.
//!
//! And then the repair has to work on that store. A refusal that names a
//! command which itself fails on the damage it names is worse than none.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::Store;
use rusqlite::Connection;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-fts-damage-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

const WIDGETS: usize = 40;

/// A store holding one generation of `WIDGETS` functions named `widget_N`, and
/// how many of its symbols a search for `widget` finds (their file's too).
fn indexed_store(dir: &Path) -> (PathBuf, usize) {
    let db = dir.join("devmap.sqlite");
    let source: String = (0..WIDGETS)
        .map(|n| format!("def widget_{n}():\n    return {n}\n\n"))
        .collect();
    let extractions = vec![devmap_extract::extract_file("src/widgets.py", &source)];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open(&db).unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let found = store.search_page("widget", 100).unwrap().unwrap().total as usize;
    assert!(
        found > WIDGETS,
        "fixture precondition: every widget and its file are searchable, found {found}"
    );
    (db, found)
}

fn assert_names_the_index(label: &str, message: &str) {
    assert!(
        message.contains("full-text index") && message.contains("devmap repair --fts"),
        "{label}: the report must name the full-text index and `devmap repair --fts`, \
         got: {message}"
    );
}

fn assert_repair_restores(db: &Path, found: usize) {
    Store::open(db)
        .unwrap()
        .repair_fts()
        .expect("`devmap repair --fts` must work on the damage it is named for");
    let reader = Store::open_read_only(db).unwrap();
    let page = reader.search_page("widget", 100).unwrap().unwrap();
    assert_eq!(page.total as usize, found, "repair must restore every row");
    let status = reader.status(&db.to_string_lossy()).unwrap();
    assert!(
        status
            .degraded_reason
            .as_deref()
            .is_none_or(|reason| !reason.contains("full-text index")),
        "a repaired index must not still be reported damaged: {:?}",
        status.degraded_reason
    );
}

#[test]
fn an_unreadable_index_is_named_by_search_and_status_and_repaired() {
    let dir = tmp_dir("unreadable");
    let (db, found) = indexed_store(&dir);
    {
        let conn = Connection::open(&db).unwrap();
        // Id 10 is FTS5's structure record: the list of segments every query
        // starts from. Garbage there fails every MATCH.
        let changed = conn
            .execute(
                "UPDATE nodes_fts_data SET block = X'FFFFFFFFFFFFFFFF' WHERE id = 10",
                [],
            )
            .unwrap();
        assert_eq!(
            changed, 1,
            "fixture precondition: the structure record exists"
        );
    }

    let reader = Store::open_read_only(&db).unwrap();
    let error = reader
        .search_page("widget", 100)
        .expect_err("a search over an unreadable index must not answer");
    assert_names_the_index("search", &error.to_string());

    let status = reader.status(&db.to_string_lossy()).unwrap();
    let reason = status
        .degraded_reason
        .clone()
        .expect("status must report an unreadable index as degraded");
    assert_names_the_index("status", &reason);
    assert!(
        !status.is_fresh(),
        "a store with an unreadable index is not fresh"
    );
    drop(reader);

    assert_repair_restores(&db, found);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_partially_lost_index_is_named_by_status_and_repaired() {
    let dir = tmp_dir("partial");
    let (db, found) = indexed_store(&dir);
    let lost = found / 2;
    {
        let conn = Connection::open(&db).unwrap();
        let removed = conn
            .execute(
                "DELETE FROM nodes_fts WHERE rowid IN (
                     SELECT rowid_ref FROM nodes_fts_map ORDER BY rowid_ref LIMIT ?1)",
                [lost as i64],
            )
            .unwrap();
        assert_eq!(removed, lost, "fixture precondition: half the rows go");
    }

    let reader = Store::open_read_only(&db).unwrap();
    let status = reader.status(&db.to_string_lossy()).unwrap();
    let reason = status
        .degraded_reason
        .clone()
        .expect("status must report a partially lost index as degraded");
    assert_names_the_index("status", &reason);
    let symbols = status.node_count;
    assert!(
        reason.contains(&format!("{} of {symbols}", symbols - lost)),
        "the reason must say how much of the index is left: {reason}"
    );
    drop(reader);

    assert_repair_restores(&db, found);
    let _ = fs::remove_dir_all(&dir);
}
