//! The daemon must not undo the build path's honesty about refused files.
//!
//! `devmap build` counts what discovery refused — a source past the 1 MiB
//! ceiling, one that cannot be read, one whose name is not UTF-8 — and folds it
//! into the analysis, so `AnalysisStatus` comes back `Partial` and the
//! dead-code pass drops out of `extracted` confidence. The file never read may
//! hold the only call to a symbol the report is about to call dead.
//!
//! The daemon reaches the same tree through the same walker and discarded the
//! answer:
//!
//! ```ignore
//! let (whole_tree, _report) = extract_tree_cached_with_report(&self.store, &self.root)?;
//! ```
//!
//! That `_report` is the refusal list. With it dropped, the drain called plain
//! `analyze()`, which assumes nothing was refused, and then **overwrote the
//! stored generation** — including its `analysis_json`. So the build path's
//! correct `Partial` survived only until the first resync: a watcher event on
//! an unrelated file was enough to relabel the corpus clean. Because the daemon
//! is the long-running path, that is the state a repository actually sits in.
//!
//! Both directions are asserted. A marker that is always on is worth as much as
//! one that is never on, so a corpus with nothing refused must still come back
//! `Ok` after the very same drain.

use devmap_analyze::model::AnalysisStatus;
use devmap_extract::MAX_SOURCE_BYTES;
use devmap_serve::Daemon;
use devmap_store::Store;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A scratch repository named for the test using it. No `tempfile`
/// dev-dependency exists in this workspace and this does not add one.
fn scratch(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "devmap-daemon-refusal-{name}-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`) and the daemon
    // canonicalizes its root, so the fixture's must be canonical too.
    dir.canonicalize().unwrap()
}

/// `helper` is called from exactly one place. That is the point: if the file
/// holding the call goes unread, `helper` looks dead.
const LIB: &str = "def helper():\n    return 1\n";
const APP: &str = "from lib import helper\n\n\ndef main():\n    return helper()\n";

/// Make the stored payloads look like an older kernel wrote them, which is what
/// installing a new kernel over an existing store does. The drain answers a
/// stale payload by re-extracting the whole tree — the branch under test.
fn age_stored_payloads(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    // `generation_files` is a view since schema v17 and is not updatable; the
    // identity lives on the payload the current generation's rows point at.
    conn.execute(
        "UPDATE file_payloads SET analyzer_version = '0.0.9:extract-v1'
          WHERE payload_id IN (
                SELECT payload_id FROM generation_file_rows
                 WHERE generation_id = (SELECT max(id) FROM generations))",
        [],
    )
    .unwrap();
}

/// Drive a repository to the point where the next drain is a full re-extraction,
/// and return the analysis status that drain persisted.
fn status_after_full_rebuild(root: &Path) -> AnalysisStatus {
    let db_path = root.join("index.sqlite");
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.to_path_buf());

    // A first generation, written normally.
    let reader = Store::open(&db_path).unwrap();
    // Enqueued through a second handle on the same SQLite file rather than
    // through the daemon: the daemon's own store is private, and widening it
    // for a test would be a worse trade than opening the file twice.
    reader
        .enqueue_pending_paths(&[
            root.join("lib.py").to_string_lossy().into_owned(),
            root.join("app.py").to_string_lossy().into_owned(),
        ])
        .unwrap();
    daemon.drain_pending_batch().unwrap();
    assert!(
        reader.latest_generation_payload_is_current().unwrap(),
        "fixture precondition: the first generation is written by this kernel"
    );

    age_stored_payloads(&db_path);
    assert!(
        !reader.latest_generation_payload_is_current().unwrap(),
        "fixture precondition: the store must now look older than this kernel, \
         or the drain takes the incremental branch and this tests nothing"
    );

    std::fs::write(root.join("lib.py"), "def helper():\n    return 11\n").unwrap();
    reader
        .enqueue_pending_paths(&[root.join("lib.py").to_string_lossy().into_owned()])
        .unwrap();
    daemon
        .drain_pending_batch()
        .expect("an aged store must resync rather than fail forever");

    reader
        .latest_analysis()
        .unwrap()
        .expect("the drain persisted a generation")
        .status
}

#[test]
fn a_full_rebuild_that_refused_a_file_does_not_persist_a_clean_status() {
    let root = scratch("oversized");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    std::fs::write(root.join("app.py"), APP).unwrap();
    // Past the 1 MiB ceiling, so discovery refuses it by size. Nothing here
    // depends on file permissions, so it behaves the same when CI runs as root.
    std::fs::write(
        root.join("unreadable.py"),
        vec![b'x'; (MAX_SOURCE_BYTES + 1) as usize],
    )
    .unwrap();

    let status = status_after_full_rebuild(&root);

    match status {
        AnalysisStatus::Ok => panic!(
            "the drain re-walked the tree, was refused a file, and stored a status \
             that says the corpus is complete. The refused file may hold the only \
             call to a symbol this generation now reports as dead — and because the \
             daemon overwrites `analysis_json`, this also erases the `Partial` that \
             `devmap build` correctly recorded."
        ),
        AnalysisStatus::Partial { reason } | AnalysisStatus::Timeout { reason } => {
            let lower = reason.to_lowercase();
            assert!(
                lower.contains("discovery") || lower.contains("refus"),
                "the reason must name the refusal, or a reader cannot tell this \
                 from any other kind of partial analysis. Got: {reason:?}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// The OFF direction, over the identical drain.
///
/// Without it, a "fix" that marked every rebuild degraded would satisfy the test
/// above while making the status useless — and `NonSource` skips are constant
/// and everywhere, so that failure is one careless predicate away.
#[test]
fn a_full_rebuild_with_nothing_refused_stays_clean() {
    let root = scratch("clean");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    std::fs::write(root.join("app.py"), APP).unwrap();
    // Deliberately present: a README is skipped as `NonSource` on every walk.
    // If that counted as loss, no repository would ever report clean again.
    std::fs::write(root.join("README.md"), "# notes\n").unwrap();

    let status = status_after_full_rebuild(&root);

    assert!(
        matches!(status, AnalysisStatus::Ok),
        "every file this walker was meant to read, it read. Reporting degraded \
         here makes the signal worthless: got {status:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The incremental branch — the one a running daemon takes almost every time.
///
/// A full rebuild is rare: it needs a stale payload or a moved HEAD. The
/// ordinary resync carries the previous generation's extractions forward and
/// re-extracts only what changed, so it never re-walks discovery and cannot
/// count refusals itself. It then called plain `analyze()`, which *asserts*
/// there were none, and overwrote `analysis_json` with the result.
///
/// The effect is that the build path's honesty has a lifetime of one watcher
/// event. `devmap build` records `Partial`; someone saves an unrelated file;
/// the daemon relabels the corpus `Ok`. Fixing only the full-rebuild branch
/// would leave this one broken while making the first test pass — the same
/// defect, one path over.
fn first_generation_with_a_refusal(root: &Path, db_path: &Path) -> Store {
    let store = Store::open(db_path).unwrap();
    // Exactly what the CLI build path does, refusal count included.
    let (extractions, report) =
        devmap_store::extract_tree_cached_with_report(&store, root).unwrap();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    // The inventory, exactly as the CLI build path supplies it: the count a
    // consumer reads is `COUNT(*)` over these rows, and `save_generation`
    // refuses a generation whose summary claims a refusal the inventory cannot
    // name.
    let refusals = devmap_store::discovery_refusals(&report);
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(refusals.len()),
    );
    store
        .save_generation_with_metadata(
            &extractions,
            &resolution,
            &analysis,
            devmap_store::GenerationWriteOpts {
                discovery_refusals: Some(refusals),
                ..Default::default()
            },
            // The stamp the daemon's own HEAD reading produces outside a git
            // repository. Without it `head_moved` is true on the first drain
            // and the daemon takes the full-rebuild branch — which re-walks
            // discovery, and so exercises the other path entirely.
            "unavailable",
        )
        .unwrap();
    store
}

#[test]
fn an_incremental_resync_does_not_erase_a_recorded_refusal() {
    let root = scratch("incremental");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    std::fs::write(root.join("app.py"), APP).unwrap();
    std::fs::write(
        root.join("unreadable.py"),
        vec![b'x'; (MAX_SOURCE_BYTES + 1) as usize],
    )
    .unwrap();
    let db_path = root.join("index.sqlite");

    let reader = first_generation_with_a_refusal(&root, &db_path);
    let built = reader.latest_analysis().unwrap().unwrap().status;
    assert!(
        matches!(built, AnalysisStatus::Partial { .. }),
        "fixture precondition: the build path must record the refusal, or there \
         is nothing for the daemon to erase. Got {built:?}"
    );

    // An ordinary edit to an unrelated file — one watcher event.
    std::fs::write(root.join("lib.py"), "def helper():\n    return 11\n").unwrap();
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    reader
        .enqueue_pending_paths(&[root.join("lib.py").to_string_lossy().into_owned()])
        .unwrap();
    assert!(
        reader.latest_generation_payload_is_current().unwrap(),
        "fixture precondition: the payload must be current, or the drain takes \
         the full-rebuild branch and this tests the other path"
    );
    // The other half of the same precondition, and the one this fixture used to
    // be missing. `save_generation` stamps `head_sha = "unknown"`, while the
    // daemon's own HEAD reading outside a git repository produces
    // `"unavailable"` — so `head_moved` was true on every drain here and the
    // test exercised the full-rebuild branch under a comment saying it did not.
    assert_eq!(
        reader.latest_generation_head_sha().unwrap().as_deref(),
        Some("unavailable"),
        "fixture precondition: the stored stamp must equal what the daemon reads \
         here, or `head_moved` sends the drain down the full-rebuild branch"
    );
    daemon
        .drain_pending_batch()
        .expect("the resync must succeed");

    let after = reader.latest_analysis().unwrap().unwrap().status;
    assert!(
        !matches!(after, AnalysisStatus::Ok),
        "the refused file is still on disk and still unread, but a resync that \
         never looked at discovery has relabelled the corpus complete. Nothing \
         about the repository changed to justify that — only that a file was \
         saved. This is the build path's correct `Partial` being overwritten by \
         a check that did not run."
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The OFF direction for the incremental branch.
///
/// A resync of a corpus with nothing refused must still come back `Ok`.
/// Carrying a status forward unconditionally would pin the first `Partial` a
/// repository ever recorded to every generation after it, so the marker could
/// never clear even once the oversized file was deleted.
#[test]
fn an_incremental_resync_of_a_clean_corpus_stays_clean() {
    let root = scratch("incremental-clean");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    std::fs::write(root.join("app.py"), APP).unwrap();
    let db_path = root.join("index.sqlite");

    let reader = first_generation_with_a_refusal(&root, &db_path);
    assert!(
        matches!(
            reader.latest_analysis().unwrap().unwrap().status,
            AnalysisStatus::Ok
        ),
        "fixture precondition: nothing was refused here"
    );

    std::fs::write(root.join("lib.py"), "def helper():\n    return 11\n").unwrap();
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    reader
        .enqueue_pending_paths(&[root.join("lib.py").to_string_lossy().into_owned()])
        .unwrap();
    daemon
        .drain_pending_batch()
        .expect("the resync must succeed");

    let after = reader.latest_analysis().unwrap().unwrap().status;
    assert!(
        matches!(after, AnalysisStatus::Ok),
        "nothing was ever refused in this repository: {after:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
