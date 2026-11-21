//! A generation pruned between a filter's scope check and its page read is a
//! pruned generation, not an empty or unbuilt store.
//!
//! A filtered keyword read resolves the scope against one generation's files
//! and then asks the store for that generation's page. A prune that commits in
//! between left the page read looking at a store without the generation, which
//! `search` and `explore` reported as "no persisted generation is available" —
//! the sentence for a store that was never built, so a caller was told to
//! build an index that was there. The reason now names the pruned generation
//! and says to retry.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use super::{ResolutionAvailability, StoreQueryEngine};
use crate::scope::NameQueryFilter;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;

thread_local! {
    static PAUSE_FIRES: Cell<u32> = const { Cell::new(0) };
    /// How many reads the hook races: one is absorbed by `explore`'s retry.
    static RACED_READS: Cell<u32> = const { Cell::new(1) };
    static STORE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

struct PauseInstall;

impl Drop for PauseInstall {
    fn drop(&mut self) {
        devmap_store::db::set_generation_read_pause(None);
        STORE_PATH.with(|slot| *slot.borrow_mut() = None);
        RACED_READS.with(|raced| raced.set(1));
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

/// Fires inside the snapshot that reads the scope's files, on the reading
/// thread, for the first `RACED_READS` fires: a second connection commits a
/// newer generation and prunes the one being read. The reading store's mutex is held, so this opens its own.
fn replace_the_generation_from_a_second_connection() {
    let fire = PAUSE_FIRES.with(|fires| {
        fires.set(fires.get() + 1);
        fires.get()
    });
    if fire > RACED_READS.with(Cell::get) {
        return;
    }
    let path = STORE_PATH
        .with(|slot| slot.borrow().clone())
        .expect("the pause fired without a database to prune");
    let writer = devmap_store::Store::open(&path).expect("a second connection opens the store");
    save(
        &writer,
        &[(
            "src/alpha.py",
            &format!("def alpha():\n    return {fire}\n"),
        )],
    );
    writer
        .prune_generations_except_latest(1)
        .expect("the prune committed while the read was paused");
}

fn fixture(tag: &str) -> (PathBuf, PathBuf, devmap_store::Store, u32) {
    let dir = std::env::temp_dir().join(format!(
        "devmap-pruned-keyword-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("index.sqlite");
    let store = devmap_store::Store::open(&db_path).unwrap();
    let generation = save(&store, &[("src/alpha.py", "def alpha():\n    return 1\n")]);
    (dir, db_path, store, generation)
}

fn path_filter() -> NameQueryFilter {
    NameQueryFilter::new(&["src".to_string()], &[], &[])
        .unwrap()
        .expect("a non-empty filter")
}

fn assert_pruned_not_unbuilt(reason: &str, generation: Option<u32>) {
    assert!(
        !reason.contains("no persisted generation"),
        "a pruned generation was reported as an empty store: {reason:?}"
    );
    assert!(
        generation.is_none_or(|generation| reason.contains(&format!("generation {generation},")))
            && reason.contains("pruned")
            && reason.contains("retry"),
        "the reason does not name the pruned generation and say to retry: {reason:?}"
    );
}

#[test]
fn search_reports_a_generation_pruned_under_its_filter_as_pruned() {
    let (dir, db_path, store, generation) = fixture("search");
    let engine = StoreQueryEngine::new(&store);
    let filter = path_filter();
    STORE_PATH.with(|slot| *slot.borrow_mut() = Some(db_path));
    PAUSE_FIRES.with(|fires| fires.set(0));
    devmap_store::db::set_generation_read_pause(Some(
        replace_the_generation_from_a_second_connection,
    ));
    let _guard = PauseInstall;

    let response = engine
        .search_filtered(
            crate::model::Request {
                query: "alpha".to_string(),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 1,
            },
            Some(&filter),
        )
        .unwrap();

    assert!(
        PAUSE_FIRES.with(Cell::get) >= 1,
        "the pause never fired, so nothing raced"
    );
    match response.resolution {
        ResolutionAvailability::Unavailable { reason } => {
            assert_pruned_not_unbuilt(&reason, Some(generation));
        }
        ResolutionAvailability::Available => {
            panic!("the page was read from a generation the store no longer holds")
        }
    }
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explore_reports_a_generation_pruned_under_its_filter_as_pruned() {
    let (dir, db_path, store, _generation) = fixture("explore");
    // Two commits: the first is absorbed by the retry `explore` makes when the
    // index moved under it; the second is the straddle it reports.
    RACED_READS.with(|raced| raced.set(2));
    let engine = StoreQueryEngine::new(&store);
    let filter = path_filter();
    STORE_PATH.with(|slot| *slot.borrow_mut() = Some(db_path));
    PAUSE_FIRES.with(|fires| fires.set(0));
    devmap_store::db::set_generation_read_pause(Some(
        replace_the_generation_from_a_second_connection,
    ));
    let _guard = PauseInstall;

    let report = engine
        .explore_filtered("alpha", 5, 8_000, 0.0, 1, Some(&filter))
        .unwrap();

    assert!(
        PAUSE_FIRES.with(Cell::get) >= 1,
        "the pause never fired, so nothing raced"
    );
    match report.definitions.resolution {
        ResolutionAvailability::Unavailable { reason } => {
            assert_pruned_not_unbuilt(&reason, None);
        }
        ResolutionAvailability::Available => {
            panic!("the definitions were read from a generation the store no longer holds")
        }
    }
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}
