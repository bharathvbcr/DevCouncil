//! Work abandoned by its caller has to actually stop.
//!
//! The IPC layer bounds a query with `tokio::time::timeout` around a
//! `spawn_blocking` task. That releases the connection slot; it does not
//! release the work, because a blocking task cannot be aborted — dropping its
//! `JoinHandle` only detaches it. So every query that ran past the 30-second
//! deadline kept scanning the corpus on a blocking-pool thread with nobody left
//! to read the answer, and enough of them fill the pool.

use devmap_extract::extract_file;
use devmap_query::cancel::{cancelled_queries, Cancel};
use devmap_query::semantic::SemanticIndex;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::Store;
use std::time::{Duration, Instant};

/// Names sharing a term, so the whole corpus is real work to index.
fn corpus(size: usize) -> Vec<String> {
    (0..size)
        .map(|index| {
            format!("computeFreshness{index} src/pkg/mod{index}.rs::computeFreshness{index}")
        })
        .collect()
}

/// The smallest uncancelled build worth measuring a cancellation against.
/// Below it, "stopped early" is indistinguishable from scheduling noise.
const BASELINE_FLOOR: Duration = Duration::from_millis(300);

/// Where the fixture stops growing. A machine that indexes this many names
/// under the floor is not one this test can say anything on, and it says so.
const CORPUS_CAP: usize = 60_000 << 5;

/// The longest loop in the engine must stop partway through, not at the end.
///
/// Sized against its own measured baseline: if the fixture is not slow enough
/// for "stopped early" to mean anything, the test says so and fails rather than
/// passing vacuously.
///
/// The fixture grows to the machine rather than being fixed at 60,000 names:
/// that size clears the floor in the debug profile `cargo test --workspace`
/// runs, and completed in 110–178 ms under `--release`, where the vacuity
/// guard then failed the test on every run — correctly, but for a reason that
/// had nothing to do with cancellation. Doubling until the uncancelled build
/// clears the floor keeps the guard meaningful in both profiles.
#[test]
fn a_cancelled_corpus_scan_stops_partway_through() {
    let mut size = 60_000;
    let (texts, baseline) = loop {
        let texts = corpus(size);
        let started = Instant::now();
        SemanticIndex::build(&texts, &Cancel::new()).expect("an uncancelled build must finish");
        let baseline = started.elapsed();
        if baseline >= BASELINE_FLOOR || size >= CORPUS_CAP {
            break (texts, baseline);
        }
        size *= 2;
    };
    assert!(
        baseline >= BASELINE_FLOOR,
        "the fixture completes in {baseline:?} even at {size} names; too fast for \
         'stopped early' to mean anything, so this test would pass without checking anything"
    );

    let cancel = Cancel::new();
    let flipper = {
        let cancel = cancel.clone();
        let after = baseline / 8;
        std::thread::spawn(move || {
            std::thread::sleep(after);
            cancel.cancel();
        })
    };

    let before = cancelled_queries();
    let started = Instant::now();
    let outcome = SemanticIndex::build(&texts, &cancel);
    let elapsed = started.elapsed();
    flipper.join().expect("flipper panicked");

    assert!(
        outcome.is_err(),
        "a build cancelled after {:?} ran all the way to completion",
        baseline / 8
    );
    assert!(
        elapsed < baseline / 2,
        "a cancelled build took {elapsed:?} against an uncancelled {baseline:?}; \
         the loop is not consulting the flag"
    );
    assert!(
        cancelled_queries() > before,
        "an abandoned scan must be counted, or a caller cannot tell it stopped"
    );
}

/// Every query surface honours the flag, and says why it stopped.
#[test]
fn a_cancelled_engine_refuses_instead_of_answering() {
    let source = "def alpha():\n    return 1\n\n\ndef beta():\n    return alpha()\n";
    let ext = extract_file("mod.py", source);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = devmap_analyze::analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    let cancel = Cancel::new();
    cancel.cancel();
    let engine = StoreQueryEngine::new(&store).with_cancel(cancel);

    let semantic = engine
        .search_semantic("alpha", 2_000)
        .expect_err("a cancelled semantic search must not answer");
    assert!(
        semantic.to_string().contains("cancelled"),
        "the refusal must say it was cancelled: {semantic}"
    );

    let traced = engine
        .trace_between(Request {
            query: ("beta".to_string(), "alpha".to_string()),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .expect_err("a cancelled scoped trace must not answer");
    assert!(
        traced.to_string().contains("cancelled"),
        "the refusal must say it was cancelled: {traced}"
    );

    let impacted = engine
        .impact(Request {
            query: "alpha".to_string(),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .expect_err("a cancelled traversal must not answer");
    assert!(
        impacted.to_string().contains("cancelled"),
        "the refusal must say it was cancelled: {impacted}"
    );
}

/// An engine with no cancellation still answers: the flag must not become a
/// blanket refusal.
#[test]
fn an_uncancelled_engine_still_answers() {
    let source = "def alpha():\n    return 1\n";
    let ext = extract_file("mod.py", source);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = devmap_analyze::analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    let response = StoreQueryEngine::new(&store)
        .with_cancel(Cancel::new())
        .search_semantic("alpha", 2_000)
        .expect("an uncancelled engine must answer");
    assert!(response.total > 0, "the query matched nothing");
}
