//! The *first* graph question a process asks may not cost the whole generation.
//!
//! `query_work_is_bounded_by_the_answer.rs` asserts the same property for the
//! second question onward: it warms the store's per-generation edge index and
//! then measures. That is the daemon's shape. The CLI's shape is the other one
//! — a process that opens the store, asks exactly one question and exits — and
//! it was never measured, because every existing bound was taken after a
//! warm-up call that had already paid the arrival cost on the test's behalf.
//!
//! Arriving at the adjacency used to mean materialising the generation: one
//! `StoredEdge` per row with six owned `String`s in it (two file paths, two
//! symbols, the edge kind and the resolution label), then four
//! `HashMap<Box<str>, Vec<u32>>` over those rows. All of it, before the walk
//! looked at the target. On this repository's own store — 102,239 edges — a
//! cold `devmap impact` retired 1.25 billion instructions against 114 million
//! for `devmap search`, which reads no edges at all; measured in process, 63.1
//! ms of a 63.6 ms `impact` was arrival and 1.5 ms was the walk.
//!
//! The bound below is deliberately loose: **one allocation per edge in the
//! generation**. A per-edge materialisation cannot get under that however
//! carefully it is written — the row alone is several — and an arrival
//! proportional to the generation's distinct *text* clears it by an order of
//! magnitude. A tight bound would have to be retuned by whoever next touches an
//! allocation in the walk, which teaches people to raise the number rather than
//! to look at it.
//!
//! A process-wide counting allocator is process-wide, so this file holds
//! exactly one `#[test]`: a second would run concurrently in the same binary
//! and each would count the other's work.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use devmap_extract::model::ParseOutcome;
use devmap_extract::treesitter::extract_treesitter_with_budget;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

// ------------------------------------------------------- counting allocator

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// Only the handing-out entry points count: the question is how much work was
// done, not how much is still held.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_during<T>(call: impl FnOnce() -> T) -> (usize, T) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let value = call();
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    (after - before, value)
}

/// See `query_work_is_bounded_by_the_answer.rs`: a fixture sized to hold
/// thousands of edges is one loaded machine away from holding none, and the
/// assertions would then compare two empty answers while reporting a defect.
const FIXTURE_PARSE_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

/// `bulk.py` holds `BULK_FUNCTIONS` functions each calling `BULK_FAN_OUT`
/// others, which is the generation's mass; `probe.py` holds a short chain, and
/// the question is asked about a link in *that*.
///
/// Two things have to be true at once for the bound below to mean anything, and
/// they pull in opposite directions:
///
/// * **Edges must outnumber symbols**, or the fixture cannot separate an
///   arrival proportional to the generation's rows from one proportional to its
///   distinct text. This repository's own store is 102,239 edges over 17,869
///   symbols — 5.7 to one — while a call chain, where every edge introduces a
///   new symbol, is one to one and would fail this bound however the arrival is
///   written.
/// * **The answer must stay small**, or the walk's own cost swamps the
///   arrival's and the bound stops measuring arrival at all. A depth-2 walk
///   inside a dense clique reaches every node in it.
///
/// So the mass and the question live in different files: the walk never enters
/// `bulk.py`, and `bulk.py` is nearly all of the generation.
const BULK_FUNCTIONS: usize = 80;
const BULK_FAN_OUT: usize = 40;
const PROBE_LINKS: usize = 6;

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-cold-query-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The fixture, written to a store **on disk** so a second `Store` can open it
/// cold.
///
/// `open_in_memory` cannot be reopened, and a reopened `Store` is the whole
/// point here: it has no memoised edge index, exactly like the process a
/// one-shot CLI call runs in.
///
/// Returns the probe symbol to ask about — a link in `probe.py`, whose callers
/// are one short chain rather than the clique.
fn fixture_store_on_disk(path: &std::path::Path) -> String {
    let mut bulk = String::new();
    for index in 0..BULK_FUNCTIONS {
        bulk.push_str(&format!("def bulk_{index:04}(rows):\n"));
        for step in 1..=BULK_FAN_OUT {
            bulk.push_str(&format!(
                "    rows = bulk_{:04}(rows)\n",
                (index + step) % BULK_FUNCTIONS
            ));
        }
        bulk.push_str("    return rows\n\n\n");
    }

    let mut probe = String::new();
    for index in 0..PROBE_LINKS {
        probe.push_str(&format!(
            "def probe_{index}(rows):\n    return probe_{}(rows)\n\n\n",
            index + 1
        ));
    }
    probe.push_str(&format!(
        "def probe_{PROBE_LINKS}(rows):\n    return rows\n"
    ));

    let extractions = vec![
        extract_treesitter_with_budget("bulk.py", "python", &bulk, FIXTURE_PARSE_BUDGET),
        extract_treesitter_with_budget("probe.py", "python", &probe, FIXTURE_PARSE_BUDGET),
    ];
    for extraction in &extractions {
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "the fixture must be extracted completely, got {:?} for {}",
            extraction.parse_outcome,
            extraction.file_path
        );
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open(path).expect("store");
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .expect("generation writes");

    format!("probe.py::probe_{}", PROBE_LINKS - 1)
}

// ---------------------------------------------------------------- the test

#[test]
fn the_first_question_a_process_asks_is_bounded_by_its_answer() {
    let dir = tmp_dir("first-question");
    let db_path = dir.join("devmap.sqlite");
    let target = fixture_store_on_disk(&db_path);

    let generation_edges = {
        let store = Store::open_existing(&db_path)
            .expect("open")
            .expect("a store must exist");
        store.latest_edges(0.0).expect("edges").len()
    };
    assert!(
        generation_edges > 2_000,
        "the fixture must hold a generation large enough for a per-edge arrival \
         to show, got {generation_edges} edge(s)"
    );

    // A *fresh* store: nothing memoised, exactly the state a one-shot CLI
    // process is in when it opens the db and asks its single question.
    let store = Store::open_existing(&db_path)
        .expect("open")
        .expect("a store must exist");
    let engine = StoreQueryEngine::new(&store);
    let (allocations, response) = allocations_during(|| {
        engine
            .impact(Request {
                query: target.clone(),
                token_budget: 2_000,
                min_confidence: 0.0,
                max_depth: 2,
            })
            .expect("impact")
    });

    assert!(
        response.shown > 0,
        "the fixture's own symbol must have callers, or this measures a miss \
         rather than an answer (total {})",
        response.total
    );
    assert!(
        response.shown as usize * 8 < generation_edges,
        "the answer must be small next to the generation, or the bound below \
         measures the walk rather than the arrival: {} edge(s) shown of \
         {generation_edges}",
        response.shown
    );

    // The measurement itself, for anyone running this with `--nocapture`: the
    // bound below is a floor to fail against, not the number to read.
    println!(
        "cold impact: {allocations} allocation(s) over a {generation_edges}-edge \
         generation to show {} edge(s)",
        response.shown
    );

    // One allocation per edge in the generation. Materialising the generation
    // costs several — the row, its owned strings, and an adjacency entry per
    // row — so this fails loudly on the shape it exists to forbid, while an
    // arrival proportional to the generation's distinct text clears it by an
    // order of magnitude.
    let budget = generation_edges;
    assert!(
        allocations < budget,
        "the first `impact` a process asks allocated {allocations} times against \
         a {generation_edges}-edge generation to show {} edge(s) — the cold path \
         is still materialising the generation before it looks at the target \
         (budget {budget})",
        response.shown
    );

    let _ = std::fs::remove_dir_all(&dir);
}
