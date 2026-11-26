//! The sweep of a new store's consumer sidecars (`.csr`, `.artifacts.json`) runs
//! under the store's writer lock, because a seed in another process places those
//! sidecars first and the store last while holding it. A sweep that did not wait
//! for the lock deleted the CSR and artifact stamp a seed had just placed.
#![cfg(unix)]

use devmap_store::seed::sidecar;
use devmap_store::Store;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("creation-sweep-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// A store being opened while a seed (the main thread) holds the lock and has
/// placed its sidecars but not yet its store: the sidecars must survive.
#[test]
fn an_open_waits_for_a_seed_that_holds_the_lock_and_leaves_its_sidecars() {
    let dir = scratch("race");
    let db = dir.join("devmap.sqlite");
    let held = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();
    std::fs::write(sidecar(&db, ".csr"), b"placed csr").unwrap();
    std::fs::write(sidecar(&db, ".artifacts.json"), b"placed stamp").unwrap();

    let (done_tx, done_rx) = mpsc::channel();
    let opener = {
        let db = db.clone();
        std::thread::spawn(move || {
            let outcome = Store::open(&db).map(drop);
            done_tx.send(()).unwrap();
            outcome
        })
    };
    // While the seed holds the lock the open cannot complete, so its completion
    // is the violation, and it is observed as an event, not inferred from a
    // sleep. Nothing here depends on the open being slow: an open that waits
    // correctly passes however long the window; a sweep that ignored the lock
    // finishes within it and is caught.
    assert!(
        done_rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "the open completed while a seed held the writer lock"
    );

    // The seed finishes: its store lands, then it lets go.
    let staged = dir.join("devmap.sqlite.seed-1");
    drop(Store::open(&staged).unwrap());
    std::fs::rename(&staged, &db).unwrap();
    drop(held);

    opener.join().unwrap().unwrap();
    assert_eq!(
        std::fs::read(sidecar(&db, ".csr")).unwrap(),
        b"placed csr",
        "the CSR a seed placed under the lock was swept"
    );
    assert_eq!(
        std::fs::read(sidecar(&db, ".artifacts.json")).unwrap(),
        b"placed stamp",
        "the artifact stamp a seed placed under the lock was swept"
    );
}

/// `devmap build` takes the lock and then opens the store it is about to
/// create: that open must not wait on its own caller, and must still sweep.
#[test]
fn an_open_by_the_thread_holding_the_lock_does_not_wait_on_itself_and_still_sweeps() {
    let dir = scratch("self");
    let db = dir.join("devmap.sqlite");
    std::fs::write(sidecar(&db, ".csr"), b"orphan").unwrap();
    let _held = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();
    let store = Store::open(&db).expect("opening under one's own lock must not deadlock");
    drop(store);
    assert!(
        !sidecar(&db, ".csr").exists(),
        "an orphan beside a store created here is still swept"
    );
}

/// Nobody holds the lock: the orphan is swept and the lock is not left held.
#[test]
fn an_orphan_beside_a_new_store_is_swept_and_the_lock_is_released() {
    let dir = scratch("plain");
    let db = dir.join("devmap.sqlite");
    std::fs::write(sidecar(&db, ".artifacts.json"), b"orphan").unwrap();
    drop(Store::open(&db).unwrap());
    assert!(!sidecar(&db, ".artifacts.json").exists());
    drop(Store::lock_writer_at(&db, Duration::from_millis(200)).expect("lock released"));
}

/// A wait on a held lock is shown to its watcher while it runs, names the
/// holder, and ends when the watcher says so, long before the bound.
#[test]
fn a_watched_wait_reports_the_holder_and_ends_when_the_watcher_breaks_it() {
    let dir = scratch("watched");
    let db = dir.join("devmap.sqlite");
    let _held = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();
    let mut polls = 0u32;
    let mut holder = String::new();
    let started = std::time::Instant::now();
    let error = Store::lock_writer_at_watched(&db, Duration::from_secs(60), &mut |wait| {
        polls += 1;
        assert_eq!(wait.bound, Duration::from_secs(60));
        holder = wait.holder_pid();
        if polls >= 3 {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    })
    .expect_err("a broken wait is a refusal");
    assert!(error.to_string().contains("was cancelled"), "{error:#}");
    assert_eq!(polls, 3);
    assert_eq!(holder, std::process::id().to_string());
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// A store created while a seed holds the lock, with no orphan beside it, must
/// also wait: otherwise the seed's final placement replaces the empty store the
/// open made, and the open's connection is left on a deleted file.
#[test]
fn a_creation_without_orphans_also_waits_for_the_lock() {
    let dir = scratch("noorphan");
    let db = dir.join("devmap.sqlite");
    let held = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let opener = {
        let db = db.clone();
        std::thread::spawn(move || {
            let outcome = Store::open(&db).map(drop);
            done_tx.send(()).unwrap();
            outcome
        })
    };
    assert!(
        done_rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "a creation completed while a seed held the writer lock"
    );
    assert!(!db.exists(), "no store may appear while the lock is held");
    drop(held);
    opener.join().unwrap().unwrap();
    assert!(db.is_file());
}
