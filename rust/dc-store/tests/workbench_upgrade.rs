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

#[test]
fn store_upgrade_reports_what_it_did_and_a_refusing_host_cannot_run_it() {
    let (_dir, path) = profile();
    let mut refusing = Store::open(&path).unwrap();
    refusing.refuse_workbench_upgrades();
    assert_eq!(
        refusing
            .workbench_request("store.upgrade", "{}")
            .unwrap_err()
            .code,
        "schema_upgrade_required"
    );
    assert_eq!(version(&path), 11);

    let store = Store::open(&path).unwrap();
    let reply = store.workbench_request("store.upgrade", "{}").unwrap();
    let field = |p: &str| -> String {
        store
            .connection()
            .query_row(
                "SELECT CAST(json_extract(?1,?2) AS TEXT)",
                [&reply, p],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(field("$.item.from"), "11");
    assert_eq!(field("$.item.upgraded"), "1");
    assert!(
        field("$.item.backup").ends_with(".pre-schema-11.bak"),
        "{reply}"
    );
    // Again: already current, nothing to do.
    let again = store.workbench_request("store.upgrade", "{}").unwrap();
    let upgraded: i64 = store
        .connection()
        .query_row("SELECT json_extract(?1,'$.item.upgraded')", [&again], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(upgraded, 0);
}

const CWD: &str = if cfg!(windows) {
    "C:/checkout"
} else {
    "/checkout"
};

fn call(store: &Store, method: &str, input: &str) -> String {
    store
        .workbench_request(method, input)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn field(store: &Store, raw: &str, path: &str) -> String {
    store
        .connection()
        .query_row(
            "SELECT CAST(json_extract(?1,?2) AS TEXT)",
            [raw, path],
            |r| r.get(0),
        )
        .unwrap()
}

fn count(path: &Path, table: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn ci_signal_rows(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM work_attention WHERE json_extract(body,'$.kind')='run_ci_failed'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

/// A profile as a schema 14 host left it, holding a task, the `run_failed` row
/// its ended run raised, and the `run_ci_failed` row `signals.record` writes. The 14 -> 15 step moves no table,
/// so the stored shape of a profile built by this build and stamped 14 is exactly
/// what a schema 14 host would have written: only the version differs.
fn profile_with_ci_signal() -> (Scratch, PathBuf) {
    let dir = Scratch::new();
    let path = dir.path().join("profile.sqlite");
    let store = Store::open(&path).unwrap();
    call(
        &store,
        "repositories.put",
        &format!(
            r#"{{"id":"r","request_id":"repo","expected_revision":0,"name":"Repo","identity_key":"local:{CWD}/.git"}}"#
        ),
    );
    call(
        &store,
        "items.put",
        r#"{"id":"t","request_id":"task-t","expected_revision":0,"title":"Fix the build","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    call(
        &store,
        "runs.prepare",
        &format!(
            r#"{{"id":"run","request_id":"prepare","expected_revision":0,"task_id":"t","source_revision":1,"repository_id":"r","repository_revision":1,"provider":"codex","permission_mode":"ask","cwd":"{CWD}","git_dir":"{CWD}/.git","git_common_dir":"{CWD}/.git","head_oid":null}}"#
        ),
    );
    call(
        &store,
        "runs.claim",
        r#"{"id":"run","request_id":"claim","expected_revision":1,"owner_id":"host","session_id":"session"}"#,
    );
    call(
        &store,
        "runs.finish",
        r#"{"id":"run","request_id":"finish","expected_revision":2,"owner_id":"host","session_id":"session","outcome":"failed","reason":"Failed before spawn"}"#,
    );
    let recorded = call(
        &store,
        "signals.record",
        r#"{"request_id":"poll-1","run_id":"run","task_id":"t","source":"github","external_id":"9001","kind":"ci_failed","conclusion":"failure","workflow":"CI","branch":"agent/t","url":"https://github.com/o/r/actions/runs/9001","head_sha":"0123456789abcdef0123456789abcdef01234567"}"#,
    );
    assert_eq!(field(&store, &recorded, "$.item.kind"), "run_ci_failed");
    drop(store);
    set_version(&path, 14);
    (dir, path)
}

#[test]
fn a_schema_14_profile_with_a_task_and_a_ci_signal_upgrades_to_15_after_a_backup() {
    let (_dir, path) = profile_with_ci_signal();
    let store = Store::open(&path).unwrap();

    // A read does not upgrade it, and takes no backup.
    let error = store.workbench_request("attention.list", "{}").unwrap_err();
    assert_eq!(error.code, "schema_upgrade_required", "{error}");
    assert_eq!(version(&path), 14, "a read upgraded the profile");
    assert!(
        !path
            .with_file_name("profile.sqlite.pre-schema-14.bak")
            .exists(),
        "a refused read made a backup"
    );

    let reply = call(&store, "store.upgrade", "{}");
    assert_eq!(field(&store, &reply, "$.item.from"), "14", "{reply}");
    assert_eq!(field(&store, &reply, "$.item.to"), "15", "{reply}");
    assert_eq!(field(&store, &reply, "$.item.upgraded"), "1", "{reply}");
    assert!(
        field(&store, &reply, "$.item.backup").ends_with(".pre-schema-14.bak"),
        "{reply}"
    );
    assert_eq!(version(&path), 15);
    assert_eq!(version(&path), dc_store::workbench::WORKBENCH_SCHEMA);

    // The backup is the whole profile at the old version, data included.
    let backup = path.with_file_name("profile.sqlite.pre-schema-14.bak");
    assert!(backup.exists(), "no backup was taken");
    assert_eq!(version(&backup), 14);
    assert_eq!(count(&backup, "work_items"), 1);
    assert_eq!(count(&backup, "work_attention"), 2);
    assert_eq!(ci_signal_rows(&backup), 1);
    assert!(
        !path
            .with_file_name("profile.sqlite.pre-schema-14.bak.partial")
            .exists()
    );

    // The upgrade kept the task and the signal row, byte for byte in meaning.
    assert_eq!(count(&path, "work_items"), 1);
    assert_eq!(count(&path, "work_attention"), 2);
    assert_eq!(ci_signal_rows(&path), 1);
    let task = call(&store, "items.get", r#"{"id":"t"}"#);
    assert_eq!(field(&store, &task, "$.item.title"), "Fix the build");
    assert_eq!(field(&store, &task, "$.item.revision"), "1");
    let page = call(&store, "attention.list", "{}");
    assert_eq!(field(&store, &page, "$.total"), "2", "{page}");
    assert_eq!(field(&store, &page, "$.items[0].kind"), "run_ci_failed");
    assert_eq!(field(&store, &page, "$.items[0].task_id"), "t");
    assert_eq!(
        field(&store, &page, "$.items[0].signal.external_id"),
        "9001"
    );

    // Already current: nothing more to do, and no second backup.
    let again = call(&store, "store.upgrade", "{}");
    assert_eq!(field(&store, &again, "$.item.upgraded"), "0", "{again}");
}

#[test]
fn a_schema_15_profile_is_current_and_a_newer_one_is_refused_as_unsupported() {
    let (_dir, path) = profile_with_ci_signal();
    set_version(&path, 15);
    let store = Store::open(&path).unwrap();
    // Current: read and written without an upgrade or a backup.
    let page = call(&store, "attention.list", "{}");
    assert_eq!(field(&store, &page, "$.total"), "2", "{page}");
    put_workspace(&store, "five").unwrap();
    assert_eq!(version(&path), 15);
    drop(store);

    // One schema ahead of this build: an older host must not touch it, and a
    // host cannot upgrade down.
    set_version(&path, 16);
    let store = Store::open(&path).unwrap();
    for result in [
        store.workbench_request("attention.list", "{}"),
        put_workspace(&store, "six"),
        store.workbench_request("store.upgrade", "{}"),
    ] {
        assert_eq!(result.unwrap_err().code, "schema_unsupported");
    }
    assert_eq!(version(&path), 16, "a newer profile was rewritten");
    let backups = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".bak"))
        .count();
    assert_eq!(backups, 0, "a refused profile was backed up");
}
