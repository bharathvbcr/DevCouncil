//! `--db` aimed at the Python engine's `index.sqlite` is named, not told to migrate.
//!
//! The Python engine's store carries `user_version = 2`, a schema this kernel
//! has no migration for. `devmap build` against it already refuses by name —
//! "check `--db` actually names a devmap store: `.devcouncil/codeintel/index.sqlite`
//! is the Python engine's database (schema 2), not this kernel's". `status`
//! against the same file, read back from the release binary, said:
//!
//! ```text
//! "degraded_reason": "store schema is 2, this binary speaks 18; run `devmap build` to migrate it"
//! ```
//!
//! — advice that cannot work, for a file the other command already knows how
//! to name. Two commands, one file, one answer.

use std::path::PathBuf;
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-pyindex-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A file shaped like the Python engine's store: SQLite, `user_version = 2`.
fn python_index(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("index.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA user_version = 2; CREATE TABLE nodes (id INTEGER);")
        .unwrap();
    path
}

#[test]
fn status_names_the_python_store_instead_of_asking_for_a_migration() {
    let dir = scratch("status");
    let db = python_index(&dir);
    let out = Command::new(devmap())
        .args(["--json", "--db"])
        .arg(&db)
        .arg("status")
        .current_dir(&dir)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("status is one JSON line ({error}): {stdout}"));
    let reason = value["degraded_reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("Python engine") && reason.contains("index.sqlite"),
        "status must name the Python engine's store as the other command does: {reason:?}"
    );
    assert!(
        !reason.contains("migrate"),
        "there is no migration from the Python schema; advising one is a dead end: {reason:?}"
    );
    assert_eq!(value["schema_outdated"], true, "{value}");
    assert_eq!(value["is_fresh"], false, "{value}");
}

/// The ordinary outdated-schema answer is unchanged: a real devmap store one
/// rung behind still says so and still points at `devmap build`.
#[test]
fn an_older_devmap_store_is_still_told_to_migrate() {
    let dir = scratch("older");
    let db = dir.join("devmap.sqlite");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA user_version = 17; CREATE TABLE paths (id INTEGER);")
        .unwrap();
    drop(conn);
    let out = Command::new(devmap())
        .args(["--json", "--db"])
        .arg(&db)
        .arg("status")
        .current_dir(&dir)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let reason = value["degraded_reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("schema is 17") && reason.contains("devmap build"),
        "{reason:?}"
    );
}
