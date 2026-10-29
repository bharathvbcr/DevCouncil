//! A store whose pages are damaged is reported as not ready, and `devmap build`
//! moves it aside and rebuilds instead of answering "unchanged".
//!
//! Damage to a page of the full-text index's shadow tables (not an SQL-level
//! edit: the bytes of the page are overwritten, as a failing disk or a torn
//! write would) made the three commands an operator tries disagree:
//!
//! - `status` said `query_ready: true` over a store whose `search` errors;
//! - `build` compared hashes, found nothing changed and exited 0;
//! - both remedies the store's own message names failed with the same
//!   "database disk image is malformed": `repair --fts` and `build --full`.
//!
//! Every row in a store is derived from the working tree, so the one remedy
//! that cannot fail is to move the file aside. It is moved, not deleted, and
//! the result says where.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn fixture(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "devmap-damaged-store-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    let source: String = (0..30)
        .map(|n| format!("def widget_{n}():\n    return {n}\n\n"))
        .collect();
    fs::write(root.join("src/widgets.py"), source).unwrap();
    root
}

fn devmap(root: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .env("DEVMAP_AUTOSPAWN", "0")
        .args(["--json", "--progress", "never", "--db"])
        .arg(root.join("devmap.sqlite"))
        .args(args)
        .output()
        .expect("run devmap");
    let json = serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
    (
        output.status.success(),
        json,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Overwrite the one page of `table` with bytes that are not a b-tree page.
fn smash_page_of(db: &Path, table: &str) {
    use std::io::{Seek, SeekFrom, Write};
    let (root_page, page_size) = {
        let conn = rusqlite::Connection::open(db).unwrap();
        // Everything in the WAL must be in the main file before its bytes are
        // overwritten, or the damage lands in a file the reader never reads.
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        let root_page: i64 = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        (root_page, page_size)
    };
    let mut file = fs::OpenOptions::new().write(true).open(db).unwrap();
    file.seek(SeekFrom::Start(((root_page - 1) * page_size) as u64))
        .unwrap();
    file.write_all(&vec![0xA5u8; page_size as usize]).unwrap();
    file.sync_all().unwrap();
}

fn quarantined(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".corrupt-"))
        })
        .collect()
}

#[test]
fn a_smashed_search_index_page_is_not_ready_and_build_sets_the_file_aside() {
    let root = fixture("fts");
    let (ok, built, _) = devmap(&root, &["build", "."]);
    assert!(ok, "{built}");
    let (_, healthy, _) = devmap(&root, &["status"]);
    assert_eq!(healthy["query_ready"], true, "{healthy}");
    assert_eq!(healthy["search_index_damaged"], false, "{healthy}");

    smash_page_of(&root.join("devmap.sqlite"), "nodes_fts_idx");

    // `status` no longer calls a store whose `search` errors ready.
    let (_, status, _) = devmap(&root, &["status"]);
    assert_eq!(status["query_ready"], false, "{status}");
    assert_eq!(status["search_index_damaged"], true, "{status}");
    assert!(
        status["degraded_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("full-text index")),
        "{status}"
    );
    let (search_ok, _, search_err) = devmap(&root, &["search", "widget_1"]);
    assert!(!search_ok, "the premise: search fails on this store");
    assert!(search_err.contains("full-text index"), "{search_err}");

    // `build` does not answer "unchanged" over it, and does not fail: it moves
    // the file aside, says where, and rebuilds.
    let (ok, rebuilt, stderr) = devmap(&root, &["build", "."]);
    assert!(ok, "build must recover, got: {rebuilt} {stderr}");
    assert_ne!(rebuilt["unchanged"], true, "{rebuilt}");
    assert_eq!(
        rebuilt["store_recovery"]["action"], "quarantined",
        "{rebuilt}"
    );
    let moved = rebuilt["store_recovery"]["quarantined_to"]
        .as_str()
        .expect("the result names where the damaged file went");
    assert!(
        Path::new(moved).is_file(),
        "{moved} must exist, not be deleted"
    );
    assert_eq!(quarantined(&root).len(), 1, "{:?}", quarantined(&root));

    // And the store it left behind is whole.
    let (_, after, _) = devmap(&root, &["status"]);
    assert_eq!(after["query_ready"], true, "{after}");
    assert_eq!(after["search_index_damaged"], false, "{after}");
    assert_eq!(after["degraded_reason"], serde_json::Value::Null, "{after}");
    let (search_ok, hits, _) = devmap(&root, &["search", "widget_1"]);
    assert!(search_ok, "{hits}");
    assert_eq!(hits["items"][0]["symbol_name"], "widget_1", "{hits}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn build_full_recovers_from_the_damage_it_used_to_die_on() {
    let root = fixture("full");
    assert!(devmap(&root, &["build", "."]).0);
    smash_page_of(&root.join("devmap.sqlite"), "nodes_fts_idx");
    let (ok, rebuilt, stderr) = devmap(&root, &["build", ".", "--full"]);
    assert!(ok, "build --full died on the damage: {rebuilt} {stderr}");
    assert_eq!(
        rebuilt["store_recovery"]["action"], "quarantined",
        "{rebuilt}"
    );
    assert_eq!(quarantined(&root).len(), 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_table_page_is_confirmed_before_the_file_is_moved() {
    // Damage the search probe does not see: the symbol table itself. The build
    // dies reading it; the wrapper confirms with a page-by-page check and only
    // then sets the file aside.
    let root = fixture("table");
    assert!(devmap(&root, &["build", "."]).0);
    smash_page_of(&root.join("devmap.sqlite"), "generation_nodes");
    let (ok, rebuilt, stderr) = devmap(&root, &["build", ".", "--full"]);
    assert!(ok, "{rebuilt} {stderr}");
    assert_eq!(
        rebuilt["store_recovery"]["action"], "quarantined",
        "{rebuilt}"
    );
    assert!(
        rebuilt["store_recovery"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("quick_check")),
        "the reason carries the page check that confirmed it: {rebuilt}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_healthy_store_is_never_moved() {
    let root = fixture("healthy");
    assert!(devmap(&root, &["build", "."]).0);
    let (ok, again, _) = devmap(&root, &["build", "."]);
    assert!(ok);
    assert_eq!(again["unchanged"], true, "{again}");
    let (ok, full, _) = devmap(&root, &["build", ".", "--full"]);
    assert!(ok);
    assert_eq!(full["store_recovery"], serde_json::Value::Null, "{full}");
    assert!(quarantined(&root).is_empty());
    let _ = fs::remove_dir_all(&root);
}
