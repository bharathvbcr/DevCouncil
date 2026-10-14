//! The gap writers take the write lock before they read, so a concurrent
//! writer is contention the busy timeout waits out rather than an immediate
//! "database is locked".
//!
//! `gap_upsert`, `gaps_replace` and `verification_commit` each read
//! (`gap_runs`) before they write. In a deferred transaction that first read
//! pins a snapshot, and SQLite will not run the busy handler for the upgrade
//! that follows: a writer holding the lock at that moment is SQLITE_BUSY, and
//! one that committed in the window is SQLITE_BUSY_SNAPSHOT, both returned at
//! once whatever the connection's five-second timeout says.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use dc_store::Store;
use dc_store::records::{GapRow, VerificationRun};
use rusqlite::Connection;

mod support;
use support::seeded;

fn gap(id: &str, task: &str) -> GapRow {
    GapRow {
        id: id.into(),
        severity: "high".into(),
        gap_type: "test_failed".into(),
        requirement_id: None,
        task_id: Some(task.into()),
        description: format!("gap {id}"),
        evidence_json: "[]".into(),
        recommended_fix: "fix".into(),
        blocking: true,
        file: None,
        line: None,
        suggested_command: None,
        acceptance_criterion_id: None,
        expected_verification_method: None,
        stdout_path: None,
        stderr_path: None,
    }
}

/// A store created (schema, WAL) by the real binary before any connection of
/// this test touches it.
fn store_file(name: &str) -> PathBuf {
    seeded(name)
}

/// How long the test lets a blocked writer sit before releasing the lock.
/// Well inside the store's five-second busy timeout, and far longer than a
/// writer that does not wait takes to come back with an error.
const HOLD: Duration = Duration::from_millis(1500);

/// Holds the database's write lock on a second connection with a committed-to
/// write pending, runs `write` on its own store from another thread, and
/// releases the lock only after `HOLD`. A writer that waits on the busy
/// handler is still blocked when the lock is released and then succeeds; one
/// that reads first in a deferred transaction comes back before the release
/// with "database is locked".
fn write_against_a_held_lock(
    db: &Path,
    write: impl FnOnce(&Store) -> dc_store::Result<()> + Send + 'static,
) {
    // Opened before the lock is taken: opening runs the schema check, which
    // would itself wait on the holder and hide what the write does.
    let store = Store::open(db).expect("open the writer");
    let holder = Connection::open(db).expect("open the holder");
    holder
        .execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO gap_runs (task_id, run) VALUES ('HOLDER', 1)
             ON CONFLICT(task_id) DO UPDATE SET run = run + 1;",
        )
        .expect("take the write lock");

    let (done_tx, done_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        let outcome = write(&store);
        let _ = done_tx.send(());
        outcome
    });

    let finished_while_locked = done_rx.recv_timeout(HOLD).is_ok();
    holder.execute_batch("COMMIT").expect("release the lock");
    let outcome = writer.join().expect("writer thread");

    assert!(
        outcome.is_ok(),
        "a concurrent writer must be waited out, not reported: {outcome:?}"
    );
    assert!(
        !finished_while_locked,
        "the write finished while another connection held the write lock"
    );
}

#[test]
fn gaps_replace_waits_for_a_concurrent_writer_instead_of_failing() {
    let db = store_file("contend-replace");
    Store::open(&db)
        .expect("open")
        .gaps_replace("T1", &[gap("OLD", "T1")])
        .expect("seed a run so the first statement is a read of it");
    write_against_a_held_lock(&db, |store| store.gaps_replace("T1", &[gap("NEW", "T1")]));
    let (rows, _) = Store::open(&db).unwrap().gaps_list(Some("T1")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "NEW");
}

#[test]
fn gap_upsert_waits_for_a_concurrent_writer_instead_of_failing() {
    let db = store_file("contend-upsert");
    write_against_a_held_lock(&db, |store| store.gap_upsert(&gap("G1", "T1")));
    let (rows, _) = Store::open(&db).unwrap().gaps_list(Some("T1")).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn verification_commit_without_a_run_waits_for_a_concurrent_writer() {
    let db = store_file("contend-commit-norun");
    write_against_a_held_lock(&db, |store| {
        store.verification_commit(None, "T1", &[gap("G1", "T1")])
    });
    let (rows, _) = Store::open(&db).unwrap().gaps_list(Some("T1")).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn verification_commit_with_a_run_waits_for_a_concurrent_writer() {
    let db = store_file("contend-commit-run");
    write_against_a_held_lock(&db, |store| {
        let run = VerificationRun {
            id: "run-1".into(),
            task_id: "T1".into(),
            sandbox: "local".into(),
            environment_json: "{}".into(),
            commands_json: "[]".into(),
            status: "blocked".into(),
            started_at: "2026-01-01T00:00:00Z".into(),
            finished_at: None,
        };
        store.verification_commit(Some(&run), "T1", &[gap("G1", "T1")])
    });
    assert!(
        Store::open(&db)
            .unwrap()
            .run_get("run-1")
            .unwrap()
            .is_some()
    );
}

/// Two stores replacing gaps on one file, interleaved, 200 times each. Every
/// call reads `gap_runs` before writing it, so under deferred transactions the
/// other thread's commits land in that window and surface as SQLITE_BUSY or
/// SQLITE_BUSY_SNAPSHOT.
#[test]
fn two_threads_interleaving_gap_replacements_never_see_a_lock_error() {
    const ROUNDS: usize = 200;
    let db = store_file("contend-loop");
    let workers: Vec<_> = ["A", "B"]
        .into_iter()
        .map(|who| {
            let path = db.clone();
            thread::spawn(move || {
                let store = Store::open(&path).expect("open");
                let mut errors = Vec::new();
                for i in 0..ROUNDS {
                    // Both threads hit the same task half the time, so the
                    // run counter and the gap rows are genuinely contended.
                    let task = if i % 2 == 0 { "SHARED" } else { who };
                    let id = format!("{who}-{i}");
                    if let Err(e) = store.gaps_replace(task, &[gap(&id, task)]) {
                        errors.push(format!("{who} round {i}: {e}"));
                    }
                }
                errors
            })
        })
        .collect();
    let errors: Vec<String> = workers
        .into_iter()
        .flat_map(|w| w.join().expect("worker"))
        .collect();
    assert!(
        errors.is_empty(),
        "{} of {} replacements failed; first: {:?}",
        errors.len(),
        2 * ROUNDS,
        errors.first()
    );
}

/// No dc-store write path may open a deferred transaction: every one goes
/// through the crate's IMMEDIATE write-transaction helper. Test-only sources
/// (`*_tests.rs`) are exempt.
#[test]
fn no_source_file_opens_a_deferred_unchecked_transaction() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                continue;
            }
            scanned += 1;
            let text = std::fs::read_to_string(&path).expect("read source");
            for (n, line) in text.lines().enumerate() {
                if line.contains(".unchecked_transaction(") {
                    offenders.push(format!("{}:{}", path.display(), n + 1));
                }
            }
        }
    }
    assert!(scanned > 0, "no sources were scanned");
    assert!(
        offenders.is_empty(),
        "deferred transactions on a write path: {offenders:?}"
    );
}
