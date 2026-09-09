use devmap_store::{Store, CURRENT_SCHEMA_VERSION};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "devmap-embedded-reader-{}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn db(&self) -> PathBuf {
        self.0.join("devmap.sqlite")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove reader fixture");
    }
}

#[test]
fn a_reader_never_creates_a_store() {
    let temp = Scratch::new();
    assert!(Store::open_read_only(temp.db()).is_err());
    assert!(!temp.db().exists());
}

#[test]
fn even_a_writable_database_is_read_only_to_an_embedding_reader() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    let before = fs::read(temp.db()).unwrap();
    let reader = Store::open_read_only(temp.db()).unwrap();
    assert!(reader.is_read_only());
    assert_eq!(reader.latest_generation_id().unwrap(), None);
    let error = reader
        .enqueue_pending_paths(&["src/a.rs".into()])
        .unwrap_err();
    assert!(error.to_string().contains("read-only"));
    drop(reader);
    assert_eq!(before, fs::read(temp.db()).unwrap());
}

#[test]
fn a_reader_cannot_create_or_take_the_writer_lock() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    let lock_path = Store::writer_lock_path(&temp.db());
    assert!(!lock_path.exists());

    let reader = Store::open_read_only(temp.db()).unwrap();
    let error = reader
        .lock_writer(std::time::Duration::ZERO)
        .expect_err("an embedding reader took the writer lock");

    assert!(error.to_string().contains("read-only"), "{error}");
    assert!(
        !lock_path.exists(),
        "a refused reader still created {}",
        lock_path.display()
    );
}

#[test]
fn old_future_and_corrupt_stores_are_refused_without_migration() {
    for version in [0, CURRENT_SCHEMA_VERSION - 1, CURRENT_SCHEMA_VERSION + 1] {
        let temp = Scratch::new();
        drop(Store::open(temp.db()).unwrap());
        let conn = rusqlite::Connection::open(temp.db()).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        drop(conn);
        let before = fs::read(temp.db()).unwrap();
        assert!(
            Store::open_read_only(temp.db()).is_err(),
            "accepted schema {version}"
        );
        assert_eq!(before, fs::read(temp.db()).unwrap());
        assert_eq!(
            Store::stored_schema_version(temp.db()).unwrap(),
            Some(version)
        );
    }
    let temp = Scratch::new();
    fs::write(temp.db(), b"not sqlite").unwrap();
    assert!(Store::open_read_only(temp.db()).is_err());
    assert_eq!(fs::read(temp.db()).unwrap(), b"not sqlite");
}

#[test]
fn concurrent_readers_cannot_enqueue_writer_work() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let db = temp.db();
            scope.spawn(move || {
                for _ in 0..25 {
                    let reader = Store::open_read_only(&db).unwrap();
                    assert_eq!(reader.latest_generation_id().unwrap(), None);
                    assert!(reader.enqueue_pending_paths(&["src/a.rs".into()]).is_err());
                }
            });
        }
    });
    assert!(Store::open(temp.db())
        .unwrap()
        .get_pending_paths()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_before_sqlite_can_block_opening_it() {
    let temp = Scratch::new();
    let result = std::process::Command::new("mkfifo")
        .arg(temp.db())
        .status()
        .unwrap();
    assert!(result.success());
    let error = match Store::open_read_only(temp.db()) {
        Ok(_) => panic!("reader accepted a FIFO"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("regular file"));
}

#[test]
fn two_worktrees_cannot_bind_the_same_store_even_before_a_generation_exists() {
    let temp = Scratch::new();
    let a = temp.0.join("a");
    let b = temp.0.join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let store = Store::open(temp.db()).unwrap();
    store.bind_repo_root(&a).unwrap();
    assert!(store
        .bind_repo_root(&b)
        .unwrap_err()
        .to_string()
        .contains("belongs to worktree"));
    assert!(store
        .enqueue_pending_paths_under_root(&b, &["x.py".into()])
        .is_err());
    assert!(store.get_pending_paths().unwrap().is_empty());
    let reader = Store::open_read_only(temp.db()).unwrap();
    reader.validate_repo_root(&a).unwrap();
    assert!(reader.validate_repo_root(&b).is_err());
}

#[test]
fn two_hundred_fifty_six_concurrent_edit_sessions_preserve_every_unique_event() {
    let temp = Scratch::new();
    // Independent connections model independent sessions, rather than sharing
    // one Rust mutex and accidentally serializing before SQLite is reached.
    let stores: Vec<_> = (0..256).map(|_| Store::open(temp.db()).unwrap()).collect();
    let barrier = std::sync::Barrier::new(stores.len());
    std::thread::scope(|scope| {
        for (session, store) in stores.iter().enumerate() {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                let paths: Vec<_> = (0..100)
                    .map(|edit| format!("session_{session}/edit_{edit}.py"))
                    .collect();
                store.enqueue_pending_paths(&paths).unwrap();
            });
        }
    });
    let claims = stores[0].claim_pending_batch(30_000).unwrap();
    assert_eq!(claims.len(), 25_600);
    assert_eq!(
        claims
            .iter()
            .map(|claim| &claim.path)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        25_600
    );
    assert_eq!(
        stores[0].clear_claimed_pending_paths(&claims).unwrap(),
        25_600
    );
    assert!(stores[0].get_pending_paths().unwrap().is_empty());
}

#[test]
fn missing_queue_identity_is_corruption_not_an_empty_queue() {
    let temp = Scratch::new();
    let store = Store::open(temp.db()).unwrap();
    store.enqueue_pending_paths(&["pending.py".into()]).unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(temp.db()).unwrap();
    conn.execute("DELETE FROM pending_state", []).unwrap();
    drop(conn);
    assert!(
        Store::open_read_only(temp.db()).is_err(),
        "reader accepted missing queue identity"
    );
    assert!(
        Store::open(temp.db()).is_err(),
        "writer accepted missing queue identity"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_database_names_cannot_bypass_writer_ownership() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    let alias = temp.0.join("alias.sqlite");
    std::os::unix::fs::symlink(temp.db(), &alias).unwrap();
    let _owner = Store::lock_writer_at(&temp.db(), std::time::Duration::ZERO).unwrap();
    assert!(
        Store::lock_writer_at(&alias, std::time::Duration::ZERO).is_err(),
        "a symlink bypassed the active writer"
    );
}

#[test]
fn hardlinked_databases_are_refused_before_diverging_wal_or_lock_files() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    let alias = temp.0.join("alias.sqlite");
    fs::hard_link(temp.db(), &alias).unwrap();
    assert!(Store::open(&alias).is_err(), "hardlink writer was accepted");
    assert!(
        Store::open_read_only(&alias).is_err(),
        "hardlink reader could miss another name's WAL"
    );
    assert!(Store::lock_writer_at(&alias, std::time::Duration::ZERO).is_err());
}

#[cfg(unix)]
#[test]
fn a_filesystem_read_only_open_still_validates_queue_identity() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    fs::set_permissions(temp.db(), fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        Store::open(temp.db()).unwrap().is_read_only(),
        "fixture must open read-only"
    );
    // A separate fixture avoids a read-only connection leaving read-only WAL sidecars.
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    let conn = rusqlite::Connection::open(temp.db()).unwrap();
    conn.execute("DELETE FROM pending_state", []).unwrap();
    drop(conn);
    fs::set_permissions(temp.db(), fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        Store::open(temp.db()).is_err(),
        "read-only fallback skipped validation"
    );
}

#[test]
fn a_queue_burst_can_wait_out_a_long_writer_without_losing_the_edit() {
    let temp = Scratch::new();
    let store = Store::open(temp.db()).unwrap();
    let blocker = rusqlite::Connection::open(temp.db()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        store.enqueue_pending_paths(&["after-long-writer.py".into()])
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    // Longer than the ordinary connection's five-second busy timeout. Queue
    // producers need their own bounded admission budget for concurrent bursts.
    std::thread::sleep(std::time::Duration::from_secs(6));
    blocker.execute_batch("ROLLBACK").unwrap();
    task.join()
        .unwrap()
        .expect("the enqueue must survive temporary writer contention");
    let store = Store::open(temp.db()).unwrap();
    assert_eq!(
        store.get_pending_paths().unwrap(),
        vec!["after-long-writer.py"]
    );
}
