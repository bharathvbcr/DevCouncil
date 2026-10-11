//! A store that arrives with the repository is not this process's own.
//!
//! `.devcouncil/codeintel/devmap.sqlite` can be committed or copied between
//! worktrees. SQLite lets a database carry triggers and views whose SQL runs
//! with this process's authority on the next write or read: a persistence
//! trigger survives every `devmap build` and rewrites what it stores, and a
//! recursive one makes the build spin. The open must refuse such a store before
//! it reads or migrates it, and must not let schema SQL call functions that
//! have side effects.
use super::*;

fn scratch(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("devmap-{label}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A valid current-schema store with `sql` executed against it by a plain
/// SQLite connection, the way whoever shipped the file could have.
fn tampered_store(label: &str, sql: &str) -> std::path::PathBuf {
    let dir = scratch(label);
    let path = dir.join("devmap.sqlite");
    drop(Store::open(&path).expect("fresh store"));
    let conn = rusqlite::Connection::open(&path).expect("plain connection");
    conn.execute_batch(sql).expect("tamper");
    drop(conn);
    path
}

const PERSISTENCE_TRIGGER: &str = "CREATE TRIGGER inject AFTER INSERT ON generation_literals
     BEGIN UPDATE generation_literals SET value = 'INJECTED-BY-SHIPPED-STORE' WHERE rowid = NEW.rowid; END;";

const SPINNING_TRIGGER: &str = "CREATE TRIGGER spin AFTER INSERT ON generations BEGIN
     WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r; END;";

fn assert_refused(result: Result<Store>, what: &str) {
    match result {
        Ok(_) => panic!("{what}: a store carrying a trigger was opened"),
        Err(error) => assert!(
            error.to_string().contains("trigger"),
            "{what}: refused, but not for the trigger: {error}"
        ),
    }
}

#[test]
fn a_store_carrying_a_persistence_trigger_is_refused_by_every_open() {
    let path = tampered_store("trigger-persist", PERSISTENCE_TRIGGER);
    assert_refused(Store::open(&path), "open");
    assert_refused(Store::open_read_only(&path), "open_read_only");
    assert!(Store::open_existing(&path).is_err(), "open_existing");
}

#[test]
fn a_store_carrying_a_recursive_trigger_is_refused_before_any_write() {
    let path = tampered_store("trigger-spin", SPINNING_TRIGGER);
    let started = std::time::Instant::now();
    assert_refused(Store::open(&path), "open");
    assert_refused(Store::open_read_only(&path), "open_read_only");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn a_trigger_on_a_temp_or_unrelated_table_is_refused_too() {
    let path = tampered_store(
        "trigger-other",
        "CREATE TABLE unrelated(x); CREATE TRIGGER t AFTER INSERT ON unrelated BEGIN SELECT 1; END;",
    );
    assert_refused(Store::open(&path), "open");
}

#[test]
fn a_clean_store_still_opens_and_schema_sql_is_untrusted() {
    let dir = scratch("clean");
    let path = dir.join("devmap.sqlite");
    for store in [
        Store::open(&path).expect("writable"),
        Store::open_read_only(&path).expect("read-only"),
        Store::open_in_memory().expect("in memory"),
    ] {
        let conn = store.conn.lock().unwrap();
        let trusted: i64 = conn
            .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
            .unwrap();
        assert_eq!(trusted, 0, "schema SQL may call side-effecting functions");
    }
}
