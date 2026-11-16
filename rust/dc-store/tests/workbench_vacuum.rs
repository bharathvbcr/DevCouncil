//! `store.vacuum`: the explicit rewrite that returns an old profile's free
//! pages and converts it to incremental auto-vacuum. Fixtures are scratch
//! databases; nothing here opens a real profile.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use dc_store::Store;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A scratch directory removed on drop (no `tempfile` dependency here).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dc-store-vacuum-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pragma(conn: &Connection, name: &str) -> i64 {
    conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
        .unwrap()
}

fn field(store: &Store, reply: &str, path: &str) -> i64 {
    store
        .connection()
        .query_row("SELECT json_extract(?1,?2)", [reply, path], |r| r.get(0))
        .unwrap()
}

fn text(store: &Store, reply: &str, path: &str) -> String {
    store
        .connection()
        .query_row(
            "SELECT CAST(json_extract(?1,?2) AS TEXT)",
            [reply, path],
            |r| r.get(0),
        )
        .unwrap()
}

/// A profile of the kind the field has: created before `auto_vacuum` was set
/// (mode NONE), then grown and emptied so it holds a large freelist. The file
/// is created by a plain connection first, because SQLite fixes the mode when
/// the first table is written.
fn old_profile_with_free_pages() -> (Scratch, PathBuf, Store) {
    let dir = Scratch::new();
    let path = dir.0.join("profile.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch("CREATE TABLE seed(x)").unwrap();
        assert_eq!(pragma(&conn, "auto_vacuum"), 0);
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        pragma(store.connection(), "auto_vacuum"),
        0,
        "mode was changed"
    );
    store.workbench_request("workspaces.list", "{}").unwrap();
    store
        .workbench_request(
            "workspaces.put",
            r#"{"request_id":"w","id":"keep-me","expected_revision":0,"name":"keep-me","repository_ids":[]}"#,
        )
        .unwrap();
    grow_and_free(&store);
    (dir, path, store)
}

/// About 8 MiB of pages, then dropped: they land on the freelist.
fn grow_and_free(store: &Store) {
    let conn = store.connection();
    conn.execute_batch("CREATE TABLE filler(b BLOB)").unwrap();
    for _ in 0..2000 {
        conn.execute("INSERT INTO filler VALUES (zeroblob(4000))", [])
            .unwrap();
    }
    conn.execute_batch("DROP TABLE filler").unwrap();
    // Fold the WAL in so the file itself holds the pages.
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

#[cfg(unix)]
#[test]
fn a_vacuum_returns_an_old_profiles_free_pages_and_makes_it_incremental() {
    let (_dir, path, store) = old_profile_with_free_pages();
    let free_before = pragma(store.connection(), "freelist_count");
    let file_before = file_len(&path);
    assert!(free_before > 1500, "{free_before}");
    assert!(file_before > 6_000_000, "{file_before}");

    // `store.compact` cannot shrink it: the mode is NONE.
    let compacted = store.workbench_request("store.compact", "{}").unwrap();
    assert_eq!(field(&store, &compacted, "$.item.vacuum.incremental"), 0);
    assert_eq!(file_len(&path), file_before);

    let reply = store.workbench_request("store.vacuum", "{}").unwrap();
    assert_eq!(text(&store, &reply, "$.item.outcome"), "vacuumed");
    assert_eq!(field(&store, &reply, "$.item.auto_vacuum_before"), 0);
    assert_eq!(field(&store, &reply, "$.item.auto_vacuum_after"), 2);
    assert_eq!(field(&store, &reply, "$.item.free_pages_after"), 0);
    assert_eq!(text(&store, &reply, "$.item.checkpoint"), "cleared");
    let before = field(&store, &reply, "$.item.bytes_before");
    let after = field(&store, &reply, "$.item.bytes_after");
    assert!(after * 4 < before, "{before} -> {after}");
    assert_eq!(
        field(&store, &reply, "$.item.file_bytes_after"),
        i64::try_from(file_len(&path)).unwrap()
    );
    assert!(file_len(&path) * 4 < file_before, "the file did not shrink");
    assert!(field(&store, &reply, "$.item.required_bytes") > 0);
    assert!(field(&store, &reply, "$.item.available_bytes") > 0);

    // The mode is in the file, not just on this connection, and the data and
    // the journal mode survived.
    let fresh = Connection::open(&path).unwrap();
    assert_eq!(pragma(&fresh, "auto_vacuum"), 2);
    let journal: String = fresh
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    let kept: i64 = fresh
        .query_row(
            "SELECT count(*) FROM work_workspaces WHERE id='keep-me'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);

    // From now on the ordinary compaction returns pages too: no second rewrite.
    grow_and_free(&store);
    let freed = file_len(&path);
    let compacted = store.workbench_request("store.compact", "{}").unwrap();
    assert_eq!(field(&store, &compacted, "$.item.vacuum.incremental"), 1);
    assert!(file_len(&path) < freed, "incremental vacuum freed nothing");
}

#[cfg(unix)]
#[test]
fn a_profile_that_is_already_incremental_and_compact_is_not_rewritten() {
    let dir = Scratch::new();
    let path = dir.0.join("profile.sqlite");
    let store = Store::open(&path).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    assert_eq!(pragma(store.connection(), "auto_vacuum"), 2);
    // Whatever the first call finds, it leaves the profile with no free pages;
    // the second must then leave the file alone.
    store.workbench_request("store.vacuum", "{}").unwrap();
    assert_eq!(pragma(store.connection(), "freelist_count"), 0);
    let size = file_len(&path);
    let reply = store.workbench_request("store.vacuum", "{}").unwrap();
    assert_eq!(text(&store, &reply, "$.item.outcome"), "not_needed");
    assert_eq!(
        field(&store, &reply, "$.item.bytes_before"),
        field(&store, &reply, "$.item.bytes_after")
    );
    assert_eq!(file_len(&path), size);
}

#[cfg(unix)]
#[test]
fn a_vacuum_the_disk_cannot_hold_is_refused_before_a_page_moves() {
    let (_dir, path, store) = old_profile_with_free_pages();
    let size = file_len(&path);
    let before = std::fs::read(&path).unwrap();
    // No disk has this much spare: the real free-space probe is what refuses.
    let error = store
        .workbench_request("store.vacuum", r#"{"reserve_bytes":9007199254740990}"#)
        .unwrap_err();
    assert_eq!(error.code, "insufficient_disk_space", "{error}");
    assert!(error.message.contains("nothing was changed"), "{error}");
    assert_eq!(file_len(&path), size);
    assert_eq!(std::fs::read(&path).unwrap(), before, "a page moved");
    assert_eq!(pragma(store.connection(), "auto_vacuum"), 0);
}

#[test]
fn a_vacuum_never_runs_as_part_of_a_migration() {
    let dir = Scratch::new();
    let path = dir.0.join("profile.sqlite");
    let store = Store::open(&path).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    drop(store);
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE work_meta SET version=11 WHERE id=1", [])
        .unwrap();
    let store = Store::open(&path).unwrap();
    let error = store.workbench_request("store.vacuum", "{}").unwrap_err();
    assert_eq!(error.code, "schema_upgrade_required", "{error}");
    let version: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 11, "the vacuum upgraded the profile");
    let backups = std::fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".bak"))
        .count();
    assert_eq!(backups, 0, "the vacuum took a migration backup");
}

#[test]
fn a_vacuum_refuses_a_transaction_unknown_fields_and_an_in_memory_profile() {
    let (_dir, _path, store) = old_profile_with_free_pages();
    store.connection().execute_batch("BEGIN").unwrap();
    let error = store.workbench_request("store.vacuum", "{}").unwrap_err();
    store.connection().execute_batch("ROLLBACK").unwrap();
    assert_eq!(error.code, "invalid_input");
    assert!(error.message.contains("transaction"), "{error}");

    let error = store
        .workbench_request("store.vacuum", r#"{"everything":true}"#)
        .unwrap_err();
    assert_eq!(error.code, "invalid_input");

    let memory = Store::open_in_memory().unwrap();
    let error = memory.workbench_request("store.vacuum", "{}").unwrap_err();
    assert_eq!(error.code, "invalid_input");
    assert!(error.message.contains("no file"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_writer_that_holds_the_profile_blocks_the_vacuum_with_a_typed_reason() {
    let (_dir, path, store) = old_profile_with_free_pages();
    store
        .connection()
        .busy_timeout(Duration::from_millis(100))
        .unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = store.workbench_request("store.vacuum", "{}").unwrap_err();
    writer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(error.code, "vacuum_blocked", "{error}");
    assert_eq!(pragma(store.connection(), "auto_vacuum"), 0, "half applied");
    // Once the writer is gone the same request goes through.
    let reply = store.workbench_request("store.vacuum", "{}").unwrap();
    assert_eq!(text(&store, &reply, "$.item.outcome"), "vacuumed");
}

/// Free space is measured with `df`, which only unix has. Anywhere else the
/// check cannot run, and a check that could not run is a refusal, not a pass.
#[cfg(not(unix))]
#[test]
fn a_vacuum_that_cannot_measure_the_disk_is_refused() {
    let (_dir, _path, store) = old_profile_with_free_pages();
    let error = store.workbench_request("store.vacuum", "{}").unwrap_err();
    assert_eq!(error.code, "disk_space_unknown", "{error}");
    assert_eq!(pragma(store.connection(), "auto_vacuum"), 0);
}
