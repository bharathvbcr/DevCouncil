//! `savings` reads its indexed-file list from one snapshot, so a generation
//! pruned while it runs is neither an error nor an empty corpus.
//!
//! The list used to be `latest_generation_id()` followed by
//! `list_generation_paths(id)`: two snapshots, and a prune between them was
//! refused as "generation N is not in this store" even though the store held a
//! newer one. The decision (consistent with `keyword_page`, which also reads
//! one snapshot) is documented on `StoreQueryEngine::savings`.
//!
//! What these tests can and cannot tell apart: the deterministic test (the hook
//! prunes at the pause) passes on the old code as well, because the hook fires
//! inside the old `list_generation_paths` snapshot too, after its existence
//! check; no seam sits between the old `latest_generation_id()` and that call.
//! Only the stress test can catch the old window, and only by chance.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::StoreQueryEngine;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;

thread_local! {
    static PAUSE_FIRES: Cell<u32> = const { Cell::new(0) };
    static STORE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

struct PauseInstall;

impl Drop for PauseInstall {
    fn drop(&mut self) {
        devmap_store::db::set_generation_read_pause(None);
        STORE_PATH.with(|slot| *slot.borrow_mut() = None);
    }
}

fn save(store: &devmap_store::Store, files: &[(&str, &str)]) -> u32 {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap()
}

fn fixture(tag: &str) -> (PathBuf, PathBuf, devmap_store::Store) {
    let dir = std::env::temp_dir().join(format!(
        "devmap-pruned-savings-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("index.sqlite");
    let store = devmap_store::Store::open(&db_path).unwrap();
    (dir, db_path, store)
}

/// Fires inside the snapshot that reads the file list, on the reading thread:
/// a second connection commits a one-file generation and prunes the one being
/// read.
fn replace_the_generation_from_a_second_connection() {
    PAUSE_FIRES.with(|fires| fires.set(fires.get() + 1));
    let path = STORE_PATH
        .with(|slot| slot.borrow().clone())
        .expect("the pause fired without a database to prune");
    let writer = devmap_store::Store::open(&path).expect("a second connection opens the store");
    save(&writer, &[("src/only.py", "def only():\n    return 9\n")]);
    writer
        .prune_generations_except_latest(1)
        .expect("the prune committed while the read was paused");
}

#[test]
fn a_prune_during_the_file_list_read_does_not_change_or_refuse_the_report() {
    let (dir, db_path, store) = fixture("pinned");
    let first = save(
        &store,
        &[
            ("src/a.py", "def a():\n    return 1\n"),
            ("src/b.py", "def b():\n    return 2\n"),
            ("src/c.py", "def c():\n    return 3\n"),
        ],
    );
    let engine = StoreQueryEngine::new(&store);
    STORE_PATH.with(|slot| *slot.borrow_mut() = Some(db_path));
    PAUSE_FIRES.with(|fires| fires.set(0));
    devmap_store::db::set_generation_read_pause(Some(
        replace_the_generation_from_a_second_connection,
    ));
    let _guard = PauseInstall;

    let report = engine
        .savings(None, 2_000)
        .expect("a prune landing mid-call must not fail the report");

    assert_eq!(
        PAUSE_FIRES.with(Cell::get),
        1,
        "savings did not read its file list through exactly one pinned snapshot"
    );
    assert_eq!(
        report.indexed_files, 3,
        "the report is the generation that was latest when the call began"
    );
    assert!(
        store.list_generation_paths(first).is_err(),
        "the prune did not commit during the pause, so nothing raced"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_store_with_no_generation_reports_an_empty_corpus_not_an_error() {
    let (dir, _db_path, store) = fixture("empty");
    let report = StoreQueryEngine::new(&store).savings(None, 2_000).unwrap();
    assert_eq!(report.indexed_files, 0);
    assert_eq!(report.corpus_bytes, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A second connection commits and prunes in a loop while `savings` runs.
/// Not deterministic, so it asserts only what must hold on every schedule:
/// every call answers, and every answer is one whole generation (1 or 3 files).
#[test]
fn savings_answers_every_call_while_another_connection_commits_and_prunes() {
    let (dir, db_path, store) = fixture("stress");
    save(
        &store,
        &[
            ("src/a.py", "def a():\n    return 1\n"),
            ("src/b.py", "def b():\n    return 2\n"),
            ("src/c.py", "def c():\n    return 3\n"),
        ],
    );
    let stop = Arc::new(AtomicBool::new(false));
    let churn = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let writer = devmap_store::Store::open(&db_path).unwrap();
            let mut round = 0u32;
            while !stop.load(Ordering::Relaxed) {
                round += 1;
                let files = if round.is_multiple_of(2) {
                    vec![("src/only.py", "def only():\n    return 9\n")]
                } else {
                    vec![
                        ("src/a.py", "def a():\n    return 1\n"),
                        ("src/b.py", "def b():\n    return 2\n"),
                        ("src/c.py", "def c():\n    return 3\n"),
                    ]
                };
                save(&writer, &files);
                writer.prune_generations_except_latest(1).unwrap();
            }
            round
        })
    };
    let engine = StoreQueryEngine::new(&store);
    let mut seen = std::collections::BTreeSet::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut calls = 0u32;
    while std::time::Instant::now() < deadline {
        calls += 1;
        match engine.savings(None, 2_000) {
            Ok(report) => {
                seen.insert(report.indexed_files);
            }
            Err(error) => {
                stop.store(true, Ordering::Relaxed);
                let _ = churn.join();
                panic!("savings refused on call {calls} while the store only moved on: {error}");
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let rounds = churn.join().expect("the churn thread");
    assert!(rounds > 0 && calls > 0);
    assert!(
        seen.iter().all(|files| *files == 1 || *files == 3),
        "a report mixed two generations: {seen:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
