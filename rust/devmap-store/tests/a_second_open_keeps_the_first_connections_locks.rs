//! Opening a second store in one process must not strip the first one's locks.
//!
//! POSIX record locks belong to the *process and inode*, not to the descriptor
//! that took them: `close()` on any descriptor for a file releases every lock
//! the process holds on it. SQLite's unix VFS knows this and never closes a
//! descriptor of its own while another connection in the process has the file
//! open. It cannot know about descriptors opened beside it, and SQLite's own
//! "How To Corrupt An SQLite Database File" (§2.2) lists exactly that.
//!
//! `Store::open` and `Store::open_read_only` used to open the database and both
//! WAL sidecars through ordinary file handles to check their link counts and
//! modes, then drop them. In a process that already held a connection — every
//! long-lived `devmap mcp` server keeps an LRU of open stores, and a daemon or
//! a query engine opens a second handle — that drop released the live
//! connection's `-shm` read lock. Another process then saw no reader, and a
//! checkpoint overwrote pages the reader's snapshot still pointed at. The reader
//! went on answering from a mix of two generations: a latest generation whose
//! full-text rows had been pruned, and "database disk image is malformed" from
//! queries a fresh process answered cleanly from the same file.
//!
//! The assertion is the lock itself, observed from a second process, because
//! that is the deterministic half: a `TRUNCATE` checkpoint must wait for every
//! reader on an older snapshot, so it can only complete if the reader's lock is
//! gone. The reader's own view is asserted too, as the consequence.
//!
//! **What this does not cover.** The same opens also closed descriptors on the
//! *database file* — `safe_fs::resolve_file_alias` on every MCP call naming a
//! repository, and the session log's ancestry check after every query — which
//! releases the connection's SHARED lock on it. Those sites now inspect without
//! opening too, but no test here goes red for them: in WAL mode the `-shm`
//! locks also refuse each cross-process action tried against that lock (a
//! last-close cleanup, a switch out of WAL), so the loss had no observable
//! consequence to assert. That half is fixed on SQLite's documented semantics,
//! not on a demonstrated failure.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::Store;
use rusqlite::{Connection, OpenFlags};

const PROBE_DB: &str = "DEVMAP_LOCK_PROBE_DB";

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-locks-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// What the writer process saw when it tried to checkpoint past the reader.
#[derive(Debug)]
struct Checkpoint {
    busy: i64,
    log: i64,
    checkpointed: i64,
}

/// Run the writer half in a separate process and read back its checkpoint.
///
/// A separate process is the whole point: POSIX locks never conflict within one
/// process, so a writer thread here would checkpoint freely whether or not the
/// reader's lock survived.
fn checkpoint_from_another_process(db: &Path) -> Checkpoint {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "writer_process", "--ignored", "--nocapture"])
        .env(PROBE_DB, db)
        .output()
        .expect("spawn the writer process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "writer process failed: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("CHECKPOINT "))
        .unwrap_or_else(|| panic!("writer process printed no checkpoint: {stdout}"));
    let mut fields = line.split(' ').map(|field| {
        field
            .split_once('=')
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .unwrap_or_else(|| panic!("unparseable checkpoint field {field:?} in {line:?}"))
    });
    Checkpoint {
        busy: fields.next().unwrap(),
        log: fields.next().unwrap(),
        checkpointed: fields.next().unwrap(),
    }
}

/// The child: commit a write, then try to checkpoint the whole log away.
///
/// Ignored so a plain `cargo test` never runs it as a test of its own; the
/// parent selects it by name. Without the environment variable it does nothing.
#[test]
#[ignore = "child process for the lock tests in this file"]
fn writer_process() {
    let Some(db) = std::env::var_os(PROBE_DB) else {
        return;
    };
    let store = Store::open(&db).expect("writer opens the store");
    store
        .enqueue_pending_paths(&["written/by/the/other/process.rs".to_string()])
        .expect("writer commits");
    let conn = Connection::open(&db).expect("writer's checkpoint connection");
    // Short: a reader that still holds its lock makes this wait out the whole
    // timeout, and the answer — busy — is already known by then.
    conn.busy_timeout(std::time::Duration::from_millis(300))
        .unwrap();
    let (busy, log, checkpointed): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("checkpoint statement runs");
    println!("CHECKPOINT busy={busy} log={log} checkpointed={checkpointed}");
    drop(conn);
    drop(store);
}
fn count_pending(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM pending_paths", [], |row| row.get(0))
        .unwrap()
}

/// Hold a read snapshot, run `second_open`, and check the snapshot still holds.
fn the_reader_keeps_its_snapshot_across(label: &str, second_open: impl FnOnce(&Path)) {
    let dir = tmp_dir(label);
    let db = dir.join("devmap.sqlite");
    {
        let store = Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&["present/before/the/reader.rs".to_string()])
            .unwrap();
    }

    let reader = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let before = count_pending(&reader);

    second_open(&db);

    let checkpoint = checkpoint_from_another_process(&db);
    let after = count_pending(&reader);
    reader.execute_batch("COMMIT").unwrap();

    assert_eq!(
        checkpoint.busy, 1,
        "{label}: another process checkpointed the whole log (log={} frames, {} \
         checkpointed) while this process held a read transaction on an older snapshot. \
         The reader's -shm lock was released by the second open, so nothing stopped the \
         checkpoint from overwriting pages the snapshot still reads",
        checkpoint.log, checkpoint.checkpointed
    );
    assert_eq!(
        before, after,
        "{label}: a read transaction saw another process's commit appear inside it"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_second_read_only_open_keeps_the_first_connections_locks() {
    the_reader_keeps_its_snapshot_across("read-only", |db| {
        drop(Store::open_read_only(db).expect("second read-only open"));
    });
}

#[test]
fn a_second_writable_open_keeps_the_first_connections_locks() {
    the_reader_keeps_its_snapshot_across("writable", |db| {
        drop(Store::open(db).expect("second writable open"));
    });
}

/// The control: with no second open, the reader's lock is what blocks the
/// checkpoint. Without it, a checkpoint that fails for an unrelated reason
/// would make the two tests above pass having proved nothing.
#[test]
fn a_held_snapshot_blocks_another_process_checkpoint() {
    the_reader_keeps_its_snapshot_across("control", |_| {});
}
