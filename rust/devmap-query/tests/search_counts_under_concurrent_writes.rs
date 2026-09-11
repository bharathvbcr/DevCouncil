//! `search` must answer from one generation, not a mixture of two.
//!
//! `StoreQueryEngine::search` resolved "the latest generation" four separate
//! times — `latest_generation_id`, `count_search_symbols`, `search_symbols`,
//! `latest_repo_root` — each taking and releasing the connection lock on its
//! own. A writer committing between any two of them split the answer across
//! generations, and the daemon is *designed* to commit while queries run.
//!
//! `Response` states the contract that breaks: clients enforce
//! `shown + hidden == total`. When the newer generation matched more rows than
//! the older one counted, `total` (old) came back smaller than `shown` (new),
//! `total.saturating_sub(shown)` clamped `hidden` to zero, and the answer
//! claimed `truncated: false`. Measured against the pre-fix code:
//! `shown=40 hidden=0 total=1 truncated=false` — forty rows handed back under a
//! count of one, with nothing marked withheld.
//!
//! `neighbors` answers the same race by detecting a straddle and disclosing it,
//! rather than locking, and says why: holding the store lock across a whole
//! fan-out blocks the writer for too long. That reasoning is about fan-outs.
//! `search` is two indexed FTS reads plus one row, so it can afford the exact
//! answer instead of a disclosed approximation — hence `Store::search_page`.

use devmap_extract::extract_file;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::Store;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A generation holding `count` symbols whose names all match "alpha".
fn generation_of(store: &Store, count: usize) {
    let body: String = (0..count)
        .map(|index| format!("def alpha_{index}():\n    return {index}\n\n\n"))
        .collect();
    let extractions = vec![extract_file("mod.py", &body)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
}

fn search(
    engine: &StoreQueryEngine,
    query: &str,
) -> devmap_query::Response<devmap_query::SymbolHit> {
    engine
        .search(Request {
            query: query.to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .expect("search answers")
}

/// The structural guard: the count and the rows come from one pinned
/// generation, so they agree by construction rather than by timing.
///
/// This is the test that holds the fix in place. It is deterministic — it does
/// not depend on winning a race — so unlike the stress probe below, a green run
/// here means something.
#[test]
fn a_search_page_counts_the_generation_it_listed() {
    let store = Store::open_in_memory().unwrap();
    generation_of(&store, 1);
    generation_of(&store, 40);

    let page = store
        .search_page("alpha", 100)
        .unwrap()
        .expect("the store holds a generation");

    assert_eq!(
        page.generation, 2,
        "the snapshot must name the generation it read"
    );
    assert_eq!(
        page.total,
        page.rows.len() as u32,
        "the count and the rows must describe the same generation: total={} rows={}",
        page.total,
        page.rows.len()
    );
    assert_eq!(page.total, 40, "generation 2 holds 40 matching symbols");
}

/// An empty store is a different answer from an empty page, and must stay so.
#[test]
fn a_store_with_no_generation_has_no_page_at_all() {
    let store = Store::open_in_memory().unwrap();
    assert!(
        store.search_page("alpha", 100).unwrap().is_none(),
        "`None` means there is no generation to answer from; `Some` with an \
         empty page would claim a generation was searched and matched nothing"
    );
}

/// The engine-level contract, across a generation change.
#[test]
fn search_reports_the_current_generations_count() {
    let store = Store::open_in_memory().unwrap();
    generation_of(&store, 1);
    let engine = StoreQueryEngine::new(&store);

    let first = search(&engine, "alpha");
    assert_eq!(first.total, 1);
    assert_eq!(first.shown + first.hidden, first.total);

    generation_of(&store, 40);

    let second = search(&engine, "alpha");
    assert_eq!(
        second.total, 40,
        "after a new generation the count must describe it, not its predecessor"
    );
    assert_eq!(
        second.shown + second.hidden,
        second.total,
        "shown={} hidden={} total={}",
        second.shown,
        second.hidden,
        second.total
    );
}

/// A stress probe, kept because it is what found the defect — and labelled,
/// because of what it cannot do.
///
/// The race needs a commit to land in the window between two reads. Against the
/// pre-fix code this caught it in 2 of 3 runs, at 2 violations per 2,000
/// queries; the third run passed with the bug fully present. **A green run here
/// is therefore not evidence of correctness** — only a red run carries
/// information. The deterministic tests above are the actual guard; this one
/// exists to exercise the real concurrent path, and is kept short so it costs
/// little to run.
#[test]
fn concurrent_writes_never_split_a_search_answer() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    generation_of(&store, 1);

    let stop = Arc::new(AtomicBool::new(false));
    let writer_store = Arc::clone(&store);
    let writer_stop = Arc::clone(&stop);
    // Alternating corpus sizes, so a straddle shows up as a count that
    // disagrees with the list in whichever direction it lands.
    let writer = std::thread::spawn(move || {
        let mut wide = true;
        while !writer_stop.load(Ordering::Relaxed) {
            generation_of(&writer_store, if wide { 40 } else { 1 });
            wide = !wide;
        }
    });

    let engine = StoreQueryEngine::new(&store);
    let mut violations = Vec::new();
    for _ in 0..300 {
        let response = search(&engine, "alpha");
        if response.shown.saturating_add(response.hidden) != response.total {
            violations.push(format!(
                "shown={} hidden={} total={} truncated={}",
                response.shown, response.hidden, response.total, response.truncated
            ));
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();

    assert!(
        violations.is_empty(),
        "`shown + hidden == total` is the contract clients enforce, and {} answers \
         broke it by counting one generation and listing another:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}
