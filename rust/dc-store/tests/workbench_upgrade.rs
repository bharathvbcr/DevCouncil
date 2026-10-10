//! The workbench schema upgrade: one-way, so it is backed up first, never
//! triggered by a read, and refusable by a host that does not own the profile.

use dc_store::Store;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A scratch directory removed on drop (no `tempfile` dependency here).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dc-store-upgrade-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn profile() -> (Scratch, PathBuf) {
    let dir = Scratch::new();
    let path = dir.path().join("profile.sqlite");
    // A profile one step behind: the 11 -> 12 step only moves the version.
    let store = Store::open(&path).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    drop(store);
    set_version(&path, 11);
    (dir, path)
}

fn set_version(path: &Path, version: i64) {
    Connection::open(path)
        .unwrap()
        .execute("UPDATE work_meta SET version=?1 WHERE id=1", [version])
        .unwrap();
}

fn version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
        .unwrap()
}

fn put_workspace(store: &Store, id: &str) -> Result<String, dc_store::workbench::Error> {
    store.workbench_request(
        "workspaces.put",
        &format!(
            r#"{{"request_id":"ws-{id}","id":"{id}","expected_revision":0,"name":"{id}","repository_ids":[]}}"#
        ),
    )
}

#[test]
fn a_read_leaves_an_older_profile_at_its_version() {
    let (_dir, path) = profile();
    let store = Store::open(&path).unwrap();
    let error = store
        .workbench_request("workspaces.list", "{}")
        .unwrap_err();
    assert_eq!(error.code, "schema_upgrade_required", "{error}");
    assert_eq!(version(&path), 11, "a read upgraded the profile");
    assert!(
        !path
            .with_file_name("profile.sqlite.pre-schema-11.bak")
            .exists(),
        "a refused read made a backup"
    );
}

#[test]
fn a_write_backs_the_profile_up_before_upgrading_it() {
    let (_dir, path) = profile();
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "one").unwrap();
    assert_eq!(version(&path), dc_store::workbench::WORKBENCH_SCHEMA);
    let backup = path.with_file_name("profile.sqlite.pre-schema-11.bak");
    assert!(backup.exists(), "no backup was taken");
    // The backup is a whole database at the old version, without the write.
    assert_eq!(version(&backup), 11);
    let rows: i64 = Connection::open(&backup)
        .unwrap()
        .query_row("SELECT count(*) FROM work_workspaces", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    assert!(
        !path
            .with_file_name("profile.sqlite.pre-schema-11.bak.partial")
            .exists()
    );
}

#[test]
fn a_host_that_refuses_upgrades_neither_reads_nor_writes_an_older_profile() {
    let (_dir, path) = profile();
    let mut store = Store::open(&path).unwrap();
    store.refuse_workbench_upgrades();
    for result in [
        store.workbench_request("workspaces.list", "{}"),
        put_workspace(&store, "two"),
    ] {
        assert_eq!(result.unwrap_err().code, "schema_upgrade_required");
    }
    assert_eq!(version(&path), 11);
}

#[test]
fn an_upgrade_whose_backup_cannot_be_written_is_refused() {
    let (_dir, path) = profile();
    // A directory where the backup file must go.
    std::fs::create_dir(path.with_file_name("profile.sqlite.pre-schema-11.bak.partial")).unwrap();
    let store = Store::open(&path).unwrap();
    let error = put_workspace(&store, "three").unwrap_err();
    assert_eq!(error.code, "backup_failed", "{error}");
    assert_eq!(version(&path), 11, "the profile moved without a backup");
}

#[test]
fn a_current_profile_takes_no_backup_and_a_fresh_one_is_created_for_a_read() {
    let dir = Scratch::new();
    let path = dir.path().join("profile.sqlite");
    let store = Store::open(&path).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    put_workspace(&store, "four").unwrap();
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".bak"))
        .count();
    assert_eq!(backups, 0);
}
