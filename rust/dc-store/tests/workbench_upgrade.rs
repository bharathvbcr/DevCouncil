//! The workbench schema upgrade: one-way, so it is backed up first, never
//! triggered by a read, and refusable by a host that does not own the profile.

use dc_store::Store;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

mod support;

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
fn a_schema_14_profile_with_a_task_and_a_ci_signal_upgrades_to_17_after_a_backup() {
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
    assert_eq!(field(&store, &reply, "$.item.to"), "17", "{reply}");
    assert_eq!(field(&store, &reply, "$.item.upgraded"), "1", "{reply}");
    assert!(
        field(&store, &reply, "$.item.backup").ends_with(".pre-schema-14.bak"),
        "{reply}"
    );
    assert_eq!(version(&path), 17);
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
fn a_schema_17_profile_is_current_and_a_newer_one_is_refused_as_unsupported() {
    let (_dir, path) = profile_with_ci_signal();
    set_version(&path, 16);
    let store = Store::open(&path).unwrap();
    let err = store.workbench_request("attention.list", "{}").unwrap_err();
    assert_eq!(err.code, "schema_upgrade_required");
    let reply = call(&store, "store.upgrade", "{}");
    assert_eq!(field(&store, &reply, "$.item.from"), "16", "{reply}");
    assert_eq!(field(&store, &reply, "$.item.to"), "17", "{reply}");
    assert_eq!(version(&path), 17);
    let queue_tables: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('work_queue','work_queue_members','work_issue_sync','work_outbox')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(queue_tables, 4);
    drop(store);

    let store = Store::open(&path).unwrap();
    // Current: read and written without an upgrade or a backup.
    let page = call(&store, "attention.list", "{}");
    assert_eq!(field(&store, &page, "$.total"), "2", "{page}");
    put_workspace(&store, "five").unwrap();
    assert_eq!(version(&path), 17);
    drop(store);

    // One schema ahead of this build: an older host must not touch it, and a
    // host cannot upgrade down.
    set_version(&path, 18);
    let store = Store::open(&path).unwrap();
    for result in [
        store.workbench_request("attention.list", "{}"),
        put_workspace(&store, "six"),
        store.workbench_request("store.upgrade", "{}"),
    ] {
        assert_eq!(result.unwrap_err().code, "schema_unsupported");
    }
    assert_eq!(version(&path), 18, "a newer profile was rewritten");
    let backups = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".bak"))
        .filter(|e| e.file_name().to_string_lossy().contains("pre-schema-17"))
        .count();
    assert_eq!(backups, 0, "a refused profile was backed up");
}

/// Clears the backup-window hook when a test ends, even by panic.
struct ResetHook;

impl Drop for ResetHook {
    fn drop(&mut self) {
        dc_store::workbench::set_backup_window_hook(None);
    }
}

/// What a second connection reports when it tries to take the write lock
/// without waiting: `Err` when another connection holds it.
fn try_write_lock(path: &Path) -> Result<(), String> {
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    conn.busy_timeout(std::time::Duration::ZERO)
        .map_err(|error| error.to_string())?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|error| error.to_string())?;
    conn.execute_batch("ROLLBACK")
        .map_err(|error| error.to_string())
}

fn workspace_rows(path: &Path, id: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM work_workspaces WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

/// Installs a hook that records whether another connection could take the
/// write lock while the backup is being written.
fn probe_lock_in_backup_window(path: &Path) -> Arc<Mutex<Vec<Result<(), String>>>> {
    let probes = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&probes);
    let path = path.to_path_buf();
    dc_store::workbench::set_backup_window_hook(Some(Arc::new(move || {
        seen.lock().unwrap().push(try_write_lock(&path));
    })));
    probes
}

fn assert_locked_out(probes: &Mutex<Vec<Result<(), String>>>) {
    let probes = probes.lock().unwrap();
    assert_eq!(
        probes.len(),
        1,
        "the backup window ran {} times",
        probes.len()
    );
    let refused = probes[0]
        .as_ref()
        .expect_err("another connection took the write lock while the backup was written");
    assert!(
        refused.contains("locked") || refused.contains("busy"),
        "the lock probe failed for another reason: {refused}"
    );
}

#[test]
fn the_upgrade_backup_exists_check_and_copy_hold_the_write_lock() {
    let (_dir, path) = profile();
    let _reset = ResetHook;
    let probes = probe_lock_in_backup_window(&path);
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "locked").unwrap();
    assert_locked_out(&probes);
    let backup = path.with_file_name("profile.sqlite.pre-schema-11.bak");
    assert_eq!(version(&backup), 11);
    assert_eq!(version(&path), dc_store::workbench::WORKBENCH_SCHEMA);
    assert!(
        try_write_lock(&path).is_ok(),
        "the backup lock outlived the backup"
    );
}

/// Host A stops after it found no backup, still inside the backup window. Host
/// B, which will not wait for a lock, then tries its whole upgrade. A host that
/// took no lock upgrades the profile and writes its own copy, and A then
/// replaces the pre-upgrade copy with a copy of the migrated profile. With the
/// lock, B is refused until A is done, and the copy A kept is the old schema.
#[test]
fn a_second_host_cannot_replace_the_pre_upgrade_backup() {
    let (_dir, path) = profile();
    // Opened first: opening takes the write lock, which host A holds below.
    let host_b = Store::open(&path).unwrap();
    host_b
        .connection()
        .busy_timeout(std::time::Duration::ZERO)
        .unwrap();
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    let resume_rx = Mutex::new(resume_rx);
    let path_a = path.clone();
    let host_a = std::thread::spawn(move || {
        let _reset = ResetHook;
        dc_store::workbench::set_backup_window_hook(Some(Arc::new(move || {
            entered_tx.send(()).unwrap();
            resume_rx.lock().unwrap().recv().unwrap();
        })));
        let store = Store::open(&path_a).unwrap();
        put_workspace(&store, "host-a").map_err(|error| error.to_string())
    });
    entered_rx
        .recv()
        .expect("host A never reached the backup window");

    let during = put_workspace(&host_b, "host-b");
    resume_tx.send(()).unwrap();
    host_a.join().unwrap().expect("host A failed");
    let refused =
        during.expect_err("host B upgraded the profile while host A held the backup window");
    assert!(
        refused.message.contains("locked") || refused.message.contains("busy"),
        "host B was refused for another reason: {refused}"
    );

    let backup = path.with_file_name("profile.sqlite.pre-schema-11.bak");
    assert_eq!(version(&backup), 11, "the pre-upgrade copy was replaced");
    assert_eq!(workspace_rows(&backup, "host-a"), 0);
    assert_eq!(version(&path), dc_store::workbench::WORKBENCH_SCHEMA);
    assert_eq!(workspace_rows(&path, "host-a"), 1);

    // Host B, now behind a finished upgrade, leaves the copy as it was.
    let kept = std::fs::read(&backup).unwrap();
    put_workspace(&host_b, "host-b").unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), kept);
    assert_eq!(workspace_rows(&backup, "host-b"), 0);
    assert_eq!(workspace_rows(&path, "host-b"), 1);
    assert!(
        !path
            .with_file_name("profile.sqlite.pre-schema-11.bak.partial")
            .exists()
    );
}

/// A verified copy already at the name (an earlier, interrupted upgrade) is
/// kept byte for byte.
#[test]
fn an_existing_verified_backup_is_kept_rather_than_rewritten() {
    let (_dir, path) = profile();
    let backup = bak11(&path);
    std::fs::copy(&path, &backup).unwrap();
    let before = std::fs::read(&backup).unwrap();
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "kept").unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    assert_eq!(version(&path), dc_store::workbench::WORKBENCH_SCHEMA);
    assert!(try_write_lock(&path).is_ok());
    assert!(
        invalid_siblings(&path).is_empty(),
        "a verified copy was moved aside"
    );
}

/// A rollback-journal profile lets a reader in beside the writer, so the copy
/// can be read while the lock is held. The upgrade must neither deadlock nor
/// skip the backup there.
#[test]
fn an_upgrade_in_delete_journal_mode_still_backs_up_the_old_schema() {
    let (_dir, path) = profile();
    let _reset = ResetHook;
    let probes = probe_lock_in_backup_window(&path);
    let store = Store::open(&path).unwrap();
    let mode: String = store
        .connection()
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_ascii_lowercase(), "delete");
    put_workspace(&store, "delete-mode").unwrap();
    assert_locked_out(&probes);
    let backup = path.with_file_name("profile.sqlite.pre-schema-11.bak");
    assert_eq!(version(&backup), 11);
    assert_eq!(workspace_rows(&backup, "delete-mode"), 0);
    assert_eq!(workspace_rows(&path, "delete-mode"), 1);
}

/// The legacy `tasks` table is retired after a `pre-tasks-retire` copy, taken
/// by the same function. Its window must hold the lock too.
#[test]
fn the_tasks_retire_backup_holds_the_write_lock() {
    let dir = Scratch::new();
    let path = dir.path().join("profile.sqlite");
    drop(Store::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(support::LEGACY_TASKS).unwrap();
    conn.execute_batch(
        r#"INSERT INTO tasks (id, title, description, status, priority, difficulty, planned_files_json,
                              requirement_ids_json, acceptance_criterion_ids_json, expected_tests_json,
                              allowed_commands_json, forbidden_changes_json,
                              agent_appended_planned_files_json)
           VALUES ('TASK-2', 'verified one', '', 'verified', 'low', NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]');
           INSERT OR REPLACE INTO schema_version (id, version) VALUES ('singleton', 9);"#,
    )
    .unwrap();
    drop(conn);
    let _reset = ResetHook;
    let probes = probe_lock_in_backup_window(&path);
    let store = Store::open(&path).unwrap();
    assert_locked_out(&probes);
    let backup = path.with_file_name("profile.sqlite.pre-tasks-retire.bak");
    let old = Connection::open(&backup).unwrap();
    let kept: i64 = old
        .query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, 1, "the retire copy does not hold the legacy table");
    let gone: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0, "the legacy table was not retired");
}

// ---- adversarial schema-backup race cases (ft-0e6cfa7a lane A attack) ----

fn bak11(path: &Path) -> PathBuf {
    path.with_file_name("profile.sqlite.pre-schema-11.bak")
}

fn assert_valid_old_backup(path: &Path) {
    assert_eq!(version(&bak11(path)), 11);
    assert!(
        !path
            .with_file_name("profile.sqlite.pre-schema-11.bak.partial")
            .exists()
    );
}

/// TRUNCATE and PERSIST keep the rollback journal; the second connection must
/// read the pre-upgrade snapshot beside the writer lock in both.
#[test]
fn an_upgrade_in_truncate_and_persist_journal_modes_backs_up_the_old_schema() {
    for mode in ["TRUNCATE", "PERSIST"] {
        let (_dir, path) = profile();
        let _reset = ResetHook;
        let probes = probe_lock_in_backup_window(&path);
        let store = Store::open(&path).unwrap();
        let got: String = store
            .connection()
            .query_row(&format!("PRAGMA journal_mode={mode}"), [], |row| row.get(0))
            .unwrap();
        assert_eq!(got.to_ascii_uppercase(), mode);
        put_workspace(&store, "m").unwrap();
        assert_locked_out(&probes);
        assert_valid_old_backup(&path);
        assert_eq!(workspace_rows(&bak11(&path), "m"), 0, "{mode}");
        assert_eq!(workspace_rows(&path, "m"), 1, "{mode}");
    }
}

/// A host that crashed mid-copy leaves a partial file of any content behind;
/// the next host replaces it and writes a correct backup.
#[test]
fn a_stale_partial_from_a_crashed_host_is_replaced() {
    let (_dir, path) = profile();
    std::fs::write(
        path.with_file_name("profile.sqlite.pre-schema-11.bak.partial"),
        b"half a copy from a host that died",
    )
    .unwrap();
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "after-crash").unwrap();
    assert_valid_old_backup(&path);
    assert_eq!(workspace_rows(&bak11(&path), "after-crash"), 0);
}

/// A host that panics inside the window drops its connection mid-backup: the
/// lock is released, nothing carries the backup name, and the next host
/// writes the backup.
#[test]
fn a_host_that_dies_in_the_backup_window_releases_the_lock_and_leaves_no_backup() {
    let (_dir, path) = profile();
    let path_a = path.clone();
    let crashed = std::thread::spawn(move || {
        let _reset = ResetHook;
        dc_store::workbench::set_backup_window_hook(Some(Arc::new(|| {
            panic!("host A dies inside the backup window")
        })));
        let store = Store::open(&path_a).unwrap();
        let _ = put_workspace(&store, "doomed");
    })
    .join();
    assert!(crashed.is_err(), "the hook did not run");
    assert!(try_write_lock(&path).is_ok(), "the dead host kept the lock");
    assert!(!bak11(&path).exists(), "a dead host left a backup name");
    assert_eq!(version(&path), 11);
    let next = Store::open(&path).unwrap();
    put_workspace(&next, "survivor").unwrap();
    assert_valid_old_backup(&path);
    assert_eq!(workspace_rows(&bak11(&path), "survivor"), 0);
}

/// A host that will not wait is refused with backup_failed while another holds
/// the window: fail closed, no migration, no backup name.
#[test]
fn a_host_that_cannot_get_the_backup_lock_is_refused_without_migrating() {
    let (_dir, path) = profile();
    let host_b = Store::open(&path).unwrap();
    host_b
        .connection()
        .busy_timeout(std::time::Duration::ZERO)
        .unwrap();
    // Opened first: opening takes the write lock, which host A holds below.
    let mut reader = Store::open(&path).unwrap();
    reader.refuse_workbench_upgrades();
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    let resume_rx = Mutex::new(resume_rx);
    let path_a = path.clone();
    let host_a = std::thread::spawn(move || {
        let _reset = ResetHook;
        dc_store::workbench::set_backup_window_hook(Some(Arc::new(move || {
            entered_tx.send(()).unwrap();
            resume_rx.lock().unwrap().recv().unwrap();
        })));
        let store = Store::open(&path_a).unwrap();
        put_workspace(&store, "a").map_err(|error| error.to_string())
    });
    entered_rx.recv().unwrap();
    let during = put_workspace(&host_b, "b");
    let seen_version = version(&path);
    let seen_backup = bak11(&path).exists();
    // A host that refuses upgrades never waits for the lock: it is told to
    // upgrade, not blocked or reported as a store error.
    let read = reader.workbench_request("workspaces.list", "{}");
    resume_tx.send(()).unwrap();
    host_a.join().unwrap().expect("host A failed");
    let refused = during.unwrap_err();
    assert_eq!(refused.code, "backup_failed", "{refused}");
    assert_eq!(seen_version, 11, "host B migrated without a backup");
    assert!(!seen_backup, "the backup name existed before A finished");
    assert_eq!(read.unwrap_err().code, "schema_upgrade_required");
}

/// Retention on the writing host removes older copies and never the one just
/// written, and a later host that finds it prunes nothing.
#[test]
fn pruning_keeps_the_pre_upgrade_backup_it_was_called_for() {
    let (_dir, path) = profile();
    let older = path.with_file_name("profile.sqlite.pre-schema-10.bak");
    std::fs::copy(&path, &older).unwrap();
    set_version(&older, 10);
    let mut store = Store::open(&path).unwrap();
    store.set_backup_retention(dc_store::workbench::BackupRetention::keep(1).unwrap());
    put_workspace(&store, "kept").unwrap();
    assert_valid_old_backup(&path);
    assert!(!older.exists(), "the older copy was not pruned");
    let mut other = Store::open(&path).unwrap();
    other.set_backup_retention(dc_store::workbench::BackupRetention::keep(1).unwrap());
    put_workspace(&other, "later").unwrap();
    assert_valid_old_backup(&path);
}

/// Unicode, spaces and a quote in the directory name, and a symlinked profile.
#[test]
fn an_awkward_or_symlinked_profile_path_is_backed_up_beside_the_real_file() {
    let dir = Scratch::new();
    let real_dir = dir.path().join("r\u{e9}al d\u{f6}r 'q' \u{65e5}\u{672c}");
    std::fs::create_dir(&real_dir).unwrap();
    let real = real_dir.join("profile.sqlite");
    let store = Store::open(&real).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    drop(store);
    set_version(&real, 11);
    let link = dir.path().join("link.sqlite");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    for target in [&real, &link] {
        let store = Store::open(target).unwrap();
        put_workspace(&store, "awkward").unwrap();
        // SQLite resolves a symlink to the real file, so the copy sits beside it.
        let backup = real.with_file_name("profile.sqlite.pre-schema-11.bak");
        assert_eq!(version(&backup), 11, "{}", backup.display());
        assert_eq!(workspace_rows(&backup, "awkward"), 0);
        drop(store);
        if target == &real {
            // The second pass starts again from an old profile.
            std::fs::remove_file(&backup).unwrap();
            set_version(&real, 11);
            Connection::open(&real)
                .unwrap()
                .execute("DELETE FROM work_workspaces", [])
                .unwrap();
        }
    }
}

/// A profile directory that cannot be written refuses the upgrade.
#[test]
fn an_unwritable_backup_directory_refuses_the_upgrade() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path) = profile();
    let store = Store::open(&path).unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let outcome = put_workspace(&store, "ro");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    if let Err(error) = outcome {
        assert_eq!(error.code, "backup_failed", "{error}");
        assert_eq!(version(&path), 11, "the profile moved without a backup");
    } else {
        // Running as a user the mode does not bind (root): nothing to assert.
        assert_valid_old_backup(&path);
    }
}

// ---- an existing file at the backup name is trusted only when it verifies ----

/// Files the upgrade moved aside from the backup name.
fn invalid_siblings(path: &Path) -> Vec<PathBuf> {
    let prefix = "profile.sqlite.pre-";
    let mut found: Vec<PathBuf> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| {
            let name = entry.file_name().unwrap().to_str().unwrap();
            name.starts_with(prefix) && name.contains(".bak.invalid-")
        })
        .collect();
    found.sort();
    found
}

/// A zero-byte file under the backup name is not a backup. It is kept (it may
/// be someone's only copy) under another name and a real copy is written.
#[test]
fn a_zero_byte_backup_name_does_not_stand_in_for_the_backup() {
    let (_dir, path) = profile();
    std::fs::write(bak11(&path), b"").unwrap();
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "z").unwrap();
    assert_valid_old_backup(&path);
    assert_eq!(workspace_rows(&bak11(&path), "z"), 0);
    let aside = invalid_siblings(&path);
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::metadata(&aside[0]).unwrap().len(), 0);
}

/// A valid database from another timeline (schema 99) under the name.
#[test]
fn a_backup_of_the_wrong_schema_is_not_trusted() {
    let (_dir, path) = profile();
    std::fs::copy(&path, bak11(&path)).unwrap();
    set_version(&bak11(&path), 99);
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "w").unwrap();
    assert_valid_old_backup(&path);
    let aside = invalid_siblings(&path);
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(version(&aside[0]), 99, "the unusable file was changed");
}

/// Two unusable files in a row each get their own name; none is deleted.
#[test]
fn moved_aside_files_never_collide() {
    let (_dir, path) = profile();
    for round in 0..2u8 {
        std::fs::write(bak11(&path), [round + 1]).unwrap();
        set_version(&path, 11);
        let store = Store::open(&path).unwrap();
        put_workspace(&store, &format!("r{round}")).unwrap();
        drop(store);
    }
    let aside = invalid_siblings(&path);
    assert_eq!(aside.len(), 2, "{aside:?}");
    let mut contents: Vec<Vec<u8>> = aside.iter().map(|p| std::fs::read(p).unwrap()).collect();
    contents.sort();
    assert_eq!(contents, vec![vec![1u8], vec![2u8]]);
}

/// The upgrade reports what it moved aside, and retention neither counts nor
/// removes it: with room for one backup, the moved-aside file stays.
#[test]
fn a_moved_aside_file_is_reported_and_not_pruned() {
    let (_dir, path) = profile();
    std::fs::write(bak11(&path), b"").unwrap();
    let mut store = Store::open(&path).unwrap();
    store.refuse_workbench_upgrades();
    drop(store);
    let store = Store::open(&path).unwrap();
    let report = store.upgrade_workbench_profile().unwrap();
    assert_eq!(report.from, 11);
    assert_eq!(report.prune.unverified.len(), 1, "{:?}", report.prune);
    assert!(report.prune.unverified[0].contains("moved aside to"));
    assert!(report.prune.removed.is_empty());
    assert_eq!(invalid_siblings(&path).len(), 1);
    assert_valid_old_backup(&path);
}

fn legacy_profile_with_stale_retire_backup(dir: &Scratch) -> (PathBuf, PathBuf) {
    let path = dir.path().join("profile.sqlite");
    drop(Store::open(&path).unwrap());
    let stale = path.with_file_name("profile.sqlite.pre-tasks-retire.bak");
    std::fs::copy(&path, &stale).unwrap(); // a valid profile with no tasks table
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(support::LEGACY_TASKS).unwrap();
    conn.execute_batch(
        r#"INSERT INTO tasks (id, title, description, status, priority, difficulty, planned_files_json,
                              requirement_ids_json, acceptance_criterion_ids_json, expected_tests_json,
                              allowed_commands_json, forbidden_changes_json,
                              agent_appended_planned_files_json)
           VALUES ('TASK-9', 'only copy', '', 'verified', 'low', NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]');
           INSERT OR REPLACE INTO schema_version (id, version) VALUES ('singleton', 9);"#,
    )
    .unwrap();
    (path, stale)
}

fn tables_named(path: &Path, name: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name=?1",
            [name],
            |r| r.get(0),
        )
        .unwrap()
}

/// A pre-existing pre-tasks-retire.bak that holds no tasks table must not
/// license dropping the legacy table: a real copy is written first.
#[test]
fn a_stale_retire_backup_without_the_tasks_table_does_not_license_the_drop() {
    let dir = Scratch::new();
    let (path, stale) = legacy_profile_with_stale_retire_backup(&dir);
    Store::open(&path).unwrap();
    assert_eq!(
        tables_named(&path, "tasks"),
        0,
        "the legacy table was not retired"
    );
    assert_eq!(
        tables_named(&stale, "tasks"),
        1,
        "the legacy table was dropped and no copy holds it"
    );
    let aside: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("profile.sqlite.pre-tasks-retire.bak.invalid-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
}

/// A retire copy that does hold the tasks table is kept as it is.
#[test]
fn a_retire_backup_that_holds_the_tasks_table_is_kept() {
    let dir = Scratch::new();
    let path = dir.path().join("profile.sqlite");
    drop(Store::open(&path).unwrap());
    let (live, stale) = legacy_profile_with_stale_retire_backup(&dir);
    assert_eq!(live, path);
    std::fs::copy(&path, &stale).unwrap(); // now with the tasks table
    let before = std::fs::read(&stale).unwrap();
    Store::open(&path).unwrap();
    assert_eq!(tables_named(&path, "tasks"), 0);
    assert_eq!(std::fs::read(&stale).unwrap(), before);
}
