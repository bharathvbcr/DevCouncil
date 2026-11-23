//! Retention of the `<profile>.pre-schema-<n>.bak` copies an upgrade writes:
//! the newest N are kept, pruning follows only a backup this call wrote and
//! verified, and nothing that is not a verified-older `pre-schema` copy goes.

use dc_store::Store;
use dc_store::workbench::BackupRetention;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A scratch directory removed on drop (no `tempfile` dependency here).
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dc-store-backups-{}-{}",
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

/// A profile one step behind the current schema (11), so a write upgrades it
/// and takes `pre-schema-11.bak`.
fn old_profile() -> (Scratch, PathBuf) {
    let dir = Scratch::new();
    let path = dir.0.join("profile.sqlite");
    let store = Store::open(&path).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    drop(store);
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE work_meta SET version=11 WHERE id=1", [])
        .unwrap();
    (dir, path)
}

fn backup_name(version: u32) -> String {
    format!("profile.sqlite.pre-schema-{version}.bak")
}

fn backup(dir: &Scratch, version: u32) -> PathBuf {
    dir.0.join(backup_name(version))
}

/// A readable backup at `version`: a database whose `work_meta` says so.
fn plant_backup(dir: &Scratch, version: u32) {
    plant_backup_labelled(dir, version, i64::from(version));
}

/// A database named `pre-schema-<name>` that holds schema `inside`.
fn plant_backup_labelled(dir: &Scratch, name: u32, inside: i64) {
    let conn = Connection::open(backup(dir, name)).unwrap();
    conn.execute_batch(
        "CREATE TABLE work_meta (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL)",
    )
    .unwrap();
    conn.execute("INSERT INTO work_meta VALUES (1, ?1)", [inside])
        .unwrap();
}

fn plant_garbage(dir: &Scratch, version: u32) {
    std::fs::write(backup(dir, version), vec![0xA5u8; 8192]).unwrap();
}

fn put_workspace(store: &Store, id: &str) {
    store
        .workbench_request(
            "workspaces.put",
            &format!(
                r#"{{"request_id":"ws-{id}","id":"{id}","expected_revision":0,"name":"{id}","repository_ids":[]}}"#
            ),
        )
        .unwrap();
}

fn present(dir: &Scratch) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("profile.sqlite.pre-"))
        .collect();
    names.sort();
    names
}

fn names(versions: &[u32]) -> Vec<String> {
    let mut expected: Vec<String> = versions.iter().map(|v| backup_name(*v)).collect();
    expected.sort();
    expected
}

#[test]
fn an_upgrade_keeps_the_newest_two_backups_by_default_ordered_numerically() {
    let (dir, path) = old_profile();
    // 3, 9 and 10 sort the wrong way as text: "9" > "3" > "10".
    for version in [3, 9, 10] {
        plant_backup(&dir, version);
    }
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "one");
    // The one just written (11) and the newest older one (10).
    assert_eq!(present(&dir), names(&[10, 11]));
}

#[test]
fn keep_all_removes_nothing_and_keep_one_keeps_only_the_new_backup() {
    let (dir, path) = old_profile();
    for version in [8, 9, 10] {
        plant_backup(&dir, version);
    }
    let mut store = Store::open(&path).unwrap();
    store.set_backup_retention(BackupRetention::KeepAll);
    put_workspace(&store, "one");
    assert_eq!(present(&dir), names(&[8, 9, 10, 11]));

    // Back at 11 with the same planted copies, keep exactly one.
    drop(store);
    std::fs::remove_file(backup(&dir, 11)).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE work_meta SET version=11 WHERE id=1", [])
        .unwrap();
    let mut store = Store::open(&path).unwrap();
    store.set_backup_retention(BackupRetention::keep(1).unwrap());
    put_workspace(&store, "two");
    assert_eq!(present(&dir), names(&[11]));
    assert!(
        BackupRetention::keep(0).is_none(),
        "0 would delete the new backup"
    );
}

#[test]
fn a_plain_open_a_read_and_a_current_profile_never_prune() {
    let (dir, path) = old_profile();
    for version in [8, 9, 10] {
        plant_backup(&dir, version);
    }
    // A read of an older profile is refused and upgrades nothing.
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .workbench_request("workspaces.list", "{}")
            .unwrap_err()
            .code,
        "schema_upgrade_required"
    );
    drop(store);
    assert_eq!(present(&dir), names(&[8, 9, 10]));

    // Upgrade once with everything kept, then use the current profile with the
    // default setting: with no new backup being written, nothing is removed.
    let mut store = Store::open(&path).unwrap();
    store.set_backup_retention(BackupRetention::KeepAll);
    put_workspace(&store, "one");
    drop(store);
    let all = names(&[8, 9, 10, 11]);
    assert_eq!(present(&dir), all);
    let store = Store::open(&path).unwrap();
    store.workbench_request("workspaces.list", "{}").unwrap();
    put_workspace(&store, "two");
    assert_eq!(present(&dir), all);
}

#[test]
fn an_existing_backup_for_this_version_is_not_a_new_backup() {
    // An interrupted upgrade left pre-schema-11 behind; resuming it writes
    // nothing, so it prunes nothing. That copy is a copy of this profile (a
    // planted stub is not: an out-of-date copy is moved aside, not trusted).
    let (dir, path) = old_profile();
    for version in [9, 10] {
        plant_backup(&dir, version);
    }
    Connection::open(&path)
        .unwrap()
        .execute("VACUUM INTO ?1", [backup(&dir, 11).to_str().unwrap()])
        .unwrap();
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "one");
    assert_eq!(present(&dir), names(&[9, 10, 11]));
}

#[test]
fn a_backup_that_fails_verification_is_never_counted_and_never_removed() {
    let (dir, path) = old_profile();
    plant_garbage(&dir, 10); // not a database
    plant_backup_labelled(&dir, 9, 5); // a database holding another schema
    plant_backup(&dir, 8);
    plant_backup(&dir, 7);
    plant_backup(&dir, 6);
    let store = Store::open(&path).unwrap();
    let reply = store.workbench_request("store.upgrade", "{}").unwrap();
    // New (11) counts one; 10 and 9 fail and count for nothing; 8 is the
    // second kept copy; 7 and 6 are beyond the two.
    assert_eq!(present(&dir), names(&[8, 9, 10, 11]));
    let field = |path: &str| -> String {
        store
            .connection()
            .query_row(
                "SELECT CAST(json_extract(?1,?2) AS TEXT)",
                [&reply, path],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!(
        field("$.item.pruned_backups.removed[0]").ends_with(&backup_name(7)),
        "{reply}"
    );
    assert!(
        field("$.item.pruned_backups.removed[1]").ends_with(&backup_name(6)),
        "{reply}"
    );
    assert!(
        field("$.item.pruned_backups.unverified[0]").contains("pre-schema-10"),
        "{reply}"
    );
    assert!(
        field("$.item.pruned_backups.unverified[1]").contains("holds schema 5, not the 9"),
        "{reply}"
    );
}

#[test]
fn only_pre_schema_backup_files_are_candidates() {
    let (dir, path) = old_profile();
    let keep_these = [
        "profile.sqlite.pre-tasks-retire.bak",
        "profile.sqlite.pre-schema-9.bak.partial",
        "profile.sqlite.pre-schema-8.bak.old",
        "profile.sqlite.compacted",
        "other.sqlite.pre-schema-1.bak",
    ];
    for name in keep_these {
        std::fs::write(dir.0.join(name), b"not ours to delete").unwrap();
    }
    // A directory with a backup's name.
    std::fs::create_dir(backup(&dir, 2)).unwrap();
    plant_backup(&dir, 10);
    plant_backup(&dir, 9);
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "one");
    for name in keep_these {
        assert!(dir.0.join(name).exists(), "{name} was removed");
    }
    assert!(backup(&dir, 2).is_dir(), "a directory was removed");
    assert!(!backup(&dir, 9).exists(), "the real older backup stayed");
    assert!(backup(&dir, 10).exists() && backup(&dir, 11).exists());
}

#[cfg(unix)]
#[test]
fn a_symlink_with_a_backups_name_is_not_removed_or_followed() {
    let (dir, path) = old_profile();
    let target = dir.0.join("precious.txt");
    std::fs::write(&target, b"precious").unwrap();
    std::os::unix::fs::symlink(&target, backup(&dir, 3)).unwrap();
    plant_backup(&dir, 10);
    plant_backup(&dir, 9);
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "one");
    assert!(std::fs::symlink_metadata(backup(&dir, 3)).is_ok());
    assert_eq!(std::fs::read(&target).unwrap(), b"precious");
}

#[test]
fn the_explicit_upgrade_reports_what_it_pruned() {
    let (dir, path) = old_profile();
    plant_backup(&dir, 9);
    plant_backup(&dir, 10);
    let store = Store::open(&path).unwrap();
    let report = store.upgrade_workbench_profile().unwrap();
    assert_eq!(report.from, 11);
    assert_eq!(
        report
            .backup
            .as_deref()
            .map(|b| Path::new(b).file_name().unwrap().to_owned()),
        Some(backup_name(11).into())
    );
    assert_eq!(report.prune.removed.len(), 1);
    assert!(report.prune.removed[0].ends_with(&backup_name(9)));
    assert!(report.prune.unverified.is_empty() && report.prune.errors.is_empty());
}

/// Runs `store.upgrade` through the real `dcstore` binary, the way a host that
/// cannot call `set_backup_retention` reaches the setting: the environment.
fn upgrade_through_the_binary(path: &Path, setting: Option<&str>) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_dcstore"));
    command
        .arg("--db")
        .arg(path)
        .args(["work", "--method", "store.upgrade", "--input", "{}"])
        .env_remove(dc_store::workbench::KEEP_BACKUPS_ENV);
    if let Some(setting) = setting {
        command.env(dc_store::workbench::KEEP_BACKUPS_ENV, setting);
    }
    command.output().unwrap()
}

fn old_profile_with_three_older_backups() -> (Scratch, PathBuf) {
    let (dir, path) = old_profile();
    for version in [8, 9, 10] {
        plant_backup(&dir, version);
    }
    (dir, path)
}

#[test]
fn the_environment_variable_reaches_the_store_through_the_binary() {
    let (dir, path) = old_profile_with_three_older_backups();
    let out = upgrade_through_the_binary(&path, Some("all"));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(present(&dir), names(&[8, 9, 10, 11]));

    let (dir, path) = old_profile_with_three_older_backups();
    let out = upgrade_through_the_binary(&path, Some("1"));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(present(&dir), names(&[11]));
    // Each removal is named on stderr, never on the JSON stdout.
    let stderr = String::from_utf8_lossy(&out.stderr);
    for version in [8, 9, 10] {
        assert!(stderr.contains(&backup_name(version)), "{stderr}");
    }
    assert!(!String::from_utf8_lossy(&out.stdout).contains("removed superseded"));

    // Unset: the default of two.
    let (dir, path) = old_profile_with_three_older_backups();
    let out = upgrade_through_the_binary(&path, None);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(present(&dir), names(&[10, 11]));
}

#[test]
fn a_setting_that_cannot_be_read_keeps_every_backup_and_says_so() {
    for bad in ["banana", "0", ""] {
        let (dir, path) = old_profile_with_three_older_backups();
        let out = upgrade_through_the_binary(&path, Some(bad));
        assert!(out.status.success(), "{bad:?}: {out:?}");
        assert_eq!(present(&dir), names(&[8, 9, 10, 11]), "{bad:?} deleted");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(dc_store::workbench::KEEP_BACKUPS_ENV),
            "{bad:?}: {stderr}"
        );
    }
}
