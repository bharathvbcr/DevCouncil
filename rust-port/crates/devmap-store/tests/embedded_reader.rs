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
