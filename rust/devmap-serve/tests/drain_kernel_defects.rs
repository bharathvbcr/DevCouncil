//! Kernel defects K1 that live in the daemon's drain rather than the store.
//!
//! These were in `devmap-store/tests/kernel_defects.rs`, which made
//! `devmap-serve` a dev-dependency of the store. That dependency took the
//! store's own `parse` feature back on through unification, so
//! `cargo test -p devmap-store --no-default-features` never built a store test
//! without grammars. The tests drive `Daemon`, so this crate is where they
//! belong; the store-level K1 gates stay beside the store.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_serve::Daemon;
use devmap_store::Store;

#[cfg(unix)]
#[path = "support/restriction.rs"]
mod restriction;
#[cfg(unix)]
use restriction::restriction_holds;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// K1(d): a failing path must not charge an attempt to its batch-mates.
///
/// `bump_pending_attempts(&batch)` ran over the whole claimed batch *before any
/// work started*, and the acknowledgement at the end only deleted rows whose
/// attempt count was positive — so the bump was load-bearing, and every path in
/// a batch shared one fate. Any error in a later batch-wide step returned
/// before that acknowledgement, leaving every claimed path one attempt worse
/// off for a failure none of them caused. Five such rounds quarantine the whole
/// batch, which is exactly the state this repository's store was found in: 64
/// rows at `attempts = 5`, the daemon's batch limit exactly.
///
/// The batch-wide failure here is a real store-level fault, not an injected
/// one: the cross-process writer lock (K13) cannot be opened for writing, which
/// is what a permissions or filesystem fault looks like from inside the drain.
/// It is taken *after* the per-path loop and *before* the acknowledgement,
/// which is precisely the window this test exists to cover.
///
/// It used to be produced by enqueuing two spellings of one file — the absolute
/// path the watcher used and the relative one the reconcile sweep used — and
/// relying on the generation write to refuse the duplicate. That route is gone:
/// `drain_pending_batch` now keys its fresh extractions by path in a
/// `BTreeMap`, deliberately, so an overlapping batch resolves to one entry per
/// file instead of failing on every retry. The de-duplication is the newer,
/// better behaviour; only this fixture's way of provoking a failure was
/// retired by it, so the fixture moved rather than the property.
///
/// Unix only, as `restriction_holds` is: it asks whether mode bits refuse this
/// process, which is how it tells an inert fixture from a passing one.
#[test]
#[cfg(unix)]
fn k1_a_failing_path_does_not_charge_an_attempt_to_its_batch_mates() {
    let dir = tmp_dir("k1-bump-isolation");
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("good.py"), "def good():\n    return 1\n").unwrap();
    let canonical_root = root.canonicalize().unwrap();
    let db = dir.join("devmap.sqlite");

    // A path that fails in isolation: it climbs out of the daemon root, which
    // `collect_pending_path` refuses. Enqueued through the raw primitive
    // precisely because the canonicalising producer refuses it up front — this
    // is a row of the kind already sitting in stores today.
    let poison = "../escapes.py".to_string();
    let sibling = canonical_root.join("good.py").display().to_string();

    {
        let store = Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&[poison.clone(), "good.py".to_string(), sibling.clone()])
            .unwrap();
        drop(store);

        // Make the writer lock unopenable for writing. Created and chmodded
        // here rather than mid-drain because the drain offers no seam: the
        // fault has to already exist when `lock_writer` reaches it. The claim
        // cannot proceed without the lock — this touches one lock file, not
        // the database.
        let lock_path = Store::writer_lock_path(&db);
        fs::write(&lock_path, b"").unwrap();
        let mut permissions = fs::metadata(&lock_path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&lock_path, permissions).unwrap();
        // Running as a user that writes through a read-only mode makes the
        // fixture inert. That is detected here and named, rather than left
        // for the precondition assertion below to report as a failure.
        if !restriction_holds(&lock_path) {
            return;
        }

        let store = Store::open(&db).unwrap();
        let daemon = Daemon::new(store, root.clone());
        let error = daemon
            .drain_pending_batch()
            .expect_err("an unwritable writer lock must fail the batch-wide step");
        // Fails closed rather than quietly: running as a user that can write
        // through a read-only mode would make the fixture inert, and this
        // assertion is what says so instead of the test passing vacuously.
        assert!(
            error.to_string().to_lowercase().contains("permission")
                || error.to_string().contains("denied"),
            "fixture precondition — the writer lock must be what failed: {error}"
        );

        let mut permissions = fs::metadata(&lock_path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&lock_path, permissions).unwrap();
    }

    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.pending_attempts(&poison).unwrap(),
        Some(0),
        "writer admission failed before any path was examined"
    );
    assert_eq!(
        store.pending_attempts("good.py").unwrap(),
        Some(0),
        "a path that resolved cleanly must not be charged for a batch-wide \
         failure it did not cause"
    );
    assert_eq!(store.pending_attempts(&sibling).unwrap(), Some(0));

    // And with no batch-wide failure, a clean path is acknowledged outright.
    store.clear_pending_paths(&[sibling]).unwrap();
    {
        let store = Store::open(&db).unwrap();
        let daemon = Daemon::new(store, root.clone());
        assert_eq!(daemon.drain_pending_batch().unwrap(), 1);
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.pending_attempts("good.py").unwrap(),
        None,
        "the path that succeeded is gone, not merely un-bumped"
    );
    assert_eq!(store.pending_attempts(&poison).unwrap(), Some(1));
    let _ = fs::remove_dir_all(&dir);
}

/// K1(h): the drain claims a whole generation's worth of work, not 64 paths.
///
/// Every drain resolves and analyses the entire repository and commits one
/// generation, so a batch's cost barely depends on how many paths it carries.
/// At 64, a 50,000-path event — a branch switch, a large `git checkout` — took
/// roughly 780 consecutive full rebuilds at one every two seconds. The cap is a
/// bound on transaction size, not a work budget.
#[test]
fn k1_default_drain_batch_is_a_bound_not_a_budget() {
    // Read through a binding so the assertion is a runtime check of the
    // exported constant rather than one clippy folds away as trivially true.
    let limit = std::hint::black_box(devmap_serve::DEFAULT_DRAIN_BATCH_LIMIT);
    assert!(
        limit >= 8192,
        "a 64-path cap multiplies full rebuilds instead of bounding work; found {limit}"
    );
}

/// K1(c): a discovery refusal is a coverage fact, not pending work.
///
/// The connect-time sweep enqueued every `Oversized` and `Unreadable` refusal
/// "to preserve failure as durable work" — but the drain applies the *same*
/// limits, so those rows were guaranteed to fail every attempt, quarantine, and
/// then sit in the queue forever holding `is_fresh` at false. The 30 MB
/// vendored `parser.c` found stuck on this repository is the case exactly: it
/// is over `MAX_SOURCE_BYTES`, and no number of retries will shrink it.
#[test]
fn k1_connect_time_sweep_does_not_queue_refusals_it_can_never_process() {
    let dir = tmp_dir("k1-refusals");
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("good.py"), "def good():\n    return 1\n").unwrap();
    fs::write(
        root.join("vendored.c"),
        vec![b'x'; devmap_extract::MAX_SOURCE_BYTES as usize + 1],
    )
    .unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let daemon = Daemon::new(store, root.clone());
    daemon.reconcile_connect_time().expect("connect-time sweep");

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let pending = store.get_pending_paths().unwrap();
    assert_eq!(
        pending,
        vec!["good.py".to_string()],
        "only work a drain can actually do belongs in the queue"
    );
    let _ = fs::remove_dir_all(&dir);
}
