//! The adversarial store cases that need no grammar: a foreign schema, racing
//! openers of a mid-chain store, and the path-id intern.
//!
//! Split from `adversarial_store.rs`, whose other cases build their generations
//! by extracting source and so are declared `required-features = ["parse"]`.
//! The shapes hunted are that file's; see its header for Class A, Class D and
//! Bounds.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::{Store, CURRENT_SCHEMA_VERSION};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-adv-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A store this binary refuses must come back byte-identical.
///
/// `Store::open` runs `configure_connection` and `enable_wal` *before*
/// `migrate` decides whether the schema is one this binary understands. So
/// pointing any devmap command at a foreign or superseded database — the
/// Python engine's `index.sqlite` at `user_version = 2` is the live instance
/// noted in PLAN.md §3.1 Class D — converted its journal mode to WAL and left
/// `-wal`/`-shm` files beside it before printing the refusal.
///
/// `future_schema_version_is_rejected_fail_closed` in
/// `store_hardening_without_grammars.rs` checks that no *table* is created. Journal mode is not a table, so the
/// mutation that does happen was invisible to it. This asserts over the file's
/// bytes instead, which is the only statement that covers mutations nobody
/// thought to enumerate.
#[test]
fn refusing_a_foreign_schema_does_not_modify_the_file() {
    for version in [2, 99] {
        let dir = tmp_dir(&format!("refuse-{version}"));
        let db_path = dir.join("foreign.sqlite");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            // Real content, so this is a database with something to lose rather
            // than an empty file where every mutation is cheap.
            conn.execute_batch(
                "CREATE TABLE symbols (id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO symbols (name) VALUES ('alpha'), ('beta');",
            )
            .unwrap();
            conn.execute(&format!("PRAGMA user_version = {version}"), [])
                .unwrap();
        }
        let before = fs::read(&db_path).unwrap();

        let error = Store::open(&db_path)
            .err()
            .unwrap_or_else(|| panic!("schema {version} must be refused"));
        let text = error.to_string();
        assert!(
            text.contains(&CURRENT_SCHEMA_VERSION.to_string())
                && text.contains(&version.to_string()),
            "the refusal must name both versions, got: {text}"
        );

        let after = fs::read(&db_path).unwrap();
        // Reported as the first differing offset rather than as two multi-KiB
        // byte vectors: `assert_eq!` on the raw buffers prints both in full and
        // buries the one fact that matters. Offsets 18 and 19 are the SQLite
        // header's write- and read-version bytes, which is where a WAL
        // conversion shows up.
        let first_difference = before
            .iter()
            .zip(after.iter())
            .position(|(a, b)| a != b)
            .or((before.len() != after.len()).then_some(before.len().min(after.len())));
        assert_eq!(
            first_difference, None,
            "opening a schema-{version} store mutated it before refusing it \
             (first differing byte offset above; 18/19 are the header's \
             write/read version, i.e. a WAL conversion). A refusal that has \
             already rewritten the database header is not fail-closed"
        );
        for sidecar in ["foreign.sqlite-wal", "foreign.sqlite-shm"] {
            assert!(
                !dir.join(sidecar).exists(),
                "refusing schema {version} left {sidecar} beside the store"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Two processes migrating the same store from a mid-chain version must both
/// succeed, and must leave exactly one schema behind.
///
/// SC28 fixed this for `version == 0` by re-reading `PRAGMA user_version`
/// inside the write transaction. Every later step of the chain still samples
/// the version *outside* any transaction, so two openers of a v7 store both
/// observe 7, both enter the `if version == 7` block, and the loser applies a
/// migration on top of a database the winner already migrated. The steps are
/// individually probed for idempotency, so the exposure is that the version
/// stamp and the applied schema can disagree — this asserts they cannot.
#[test]
fn concurrent_openers_of_a_mid_chain_store_converge_on_one_schema() {
    let dir = tmp_dir("mid-chain-race");
    let db_path = dir.join("v7.sqlite");
    // Build a v12 store, then rewind the stamp so the chain has work to do.
    // Rewinding rather than hand-writing a v7 schema keeps this test about the
    // *race*, not about reproducing a historical DDL by hand.
    {
        let store = Store::open(&db_path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 7", []).unwrap();
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let path = db_path.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            Store::open(&path).map(|_| ()).map_err(|e| e.to_string())
        }));
    }
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let failures: Vec<_> = outcomes.iter().filter_map(|o| o.as_ref().err()).collect();
    assert!(
        failures.is_empty(),
        "racing openers of a mid-chain store failed: {failures:?}"
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok", "the race corrupted the store");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn audit_path_id_exhaustion_rolls_back_the_failed_intern() {
    let dir = tmp_dir("path-overflow");
    let db = dir.join("map.sqlite");
    let store = Store::open(&db).unwrap();
    store.get_or_create_path_id("before.py").unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'paths'",
        [i64::from(u32::MAX)],
    )
    .unwrap();
    for _ in 0..32 {
        assert!(store.get_or_create_path_id("overflow.py").is_err());
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM paths WHERE path = 'overflow.py'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "refusing an unrepresentable ID must not leave a persisted row"
    );
    drop(conn);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_concurrent_path_interns_reuse_one_identity() {
    let dir = tmp_dir("path-intern-stress");
    let db = dir.join("map.sqlite");
    let stores: Vec<Store> = (0..8).map(|_| Store::open(&db).unwrap()).collect();
    let start = std::sync::Barrier::new(stores.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = stores
            .into_iter()
            .map(|store| {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    (0..128)
                        .map(|step| {
                            let name = format!("path-{}.py", step % 16);
                            let id = store.get_or_create_path_id(&name).unwrap();
                            (name, id)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut identities = std::collections::BTreeMap::new();
        for handle in handles {
            for (name, id) in handle.join().unwrap() {
                if let Some(previous) = identities.insert(name, id) {
                    assert_eq!(previous, id);
                }
            }
        }
        assert_eq!(identities.len(), 16);
    });
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM paths", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        16
    );
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(conn);
    fs::remove_dir_all(dir).unwrap();
}
