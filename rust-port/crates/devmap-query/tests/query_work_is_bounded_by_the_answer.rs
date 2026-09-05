//! A query may scan the generation; it may not *allocate* per edge of it.
//!
//! Three surfaces used to do heap work proportional to the whole edge table
//! rather than to the answer, and all three were invisible to every existing
//! test because none of them changed an output:
//!
//! * `traversal_starts` resolving a path-shaped target called `path_matches`
//!   once per edge, and `path_matches` allocated to answer a boolean: two
//!   `replace`s and a `format!`, which this file measured at four heap
//!   operations per edge (200,001 over a 50,000-edge scan — `format!`'s
//!   growth is the fourth).
//! * `traversed_resolution_edges` built an owned `(String, String, String)`
//!   lookup key for every edge in the generation and dropped it immediately:
//!   150,010 allocations to select one edge out of 50,000.
//! * `neighbors` re-read and re-converted the whole edge table `2 * targets`
//!   times, once inside each `impact` and each `trace` it composed. Hoisting
//!   the load fixed the composition; the per-generation edge index in
//!   `devmap-store` fixed the sub-queries too, so the bound asserted below is
//!   now absolute (allocations against the generation's edge count) rather
//!   than a ratio between the two spellings.
//!
//! Timing would catch these, and timing is exactly what should not be asserted
//! in a test suite: it turns a shared machine under load into a red build.
//! Counting allocations does not — the counts are deterministic, and they are
//! the actual defect rather than a proxy for it. A process-wide counting
//! allocator is process-wide, so this file holds exactly one `#[test]`: two
//! would run concurrently in the same binary and each would count the other's
//! work.
//!
//! The bounds below are order-of-magnitude, not tight. They are meant to
//! separate "proportional to the answer" from "proportional to the graph", and
//! a bound that has to be retuned whenever an implementation detail moves
//! teaches maintainers to raise it rather than to look.
//!
//! Semantics are *not* asserted here. `neighbors_composition.rs` already
//! checks the composed answer field-by-field against the calls it replaces, and
//! `src/query_match.rs` differential-tests the path matcher against the
//! spelling it replaced; a fast wrong answer fails there.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use devmap_analyze::traversal::{traverse_graph, TraversalOptions};
use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, EdgeKind};
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::model::ResolvedEdge;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

// ------------------------------------------------------- counting allocator

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// Every entry point that can hand out fresh memory increments the counter;
// `dealloc` does not, because the question is how much work was done, not how
// much is still held.
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
    let answer = call();
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    (after - before, answer)
}

// ------------------------------------------------------------------ fixtures

const EDGES: usize = 50_000;

/// A generation-shaped edge list: distinct files, distinct symbols, one call
/// edge each. Synthesised rather than extracted, because the property under
/// test is about the *scan* and 50,000 real edges would make this a slow test
/// for no extra coverage.
fn synthetic_edges(count: usize) -> Vec<ResolvedEdge> {
    (0..count)
        .map(|index| ResolvedEdge {
            source_file: format!("pkg{}/mod_{index:05}.py", index % 40),
            target_file: format!("pkg{}/mod_{:05}.py", (index + 1) % 40, index + 1),
            source_symbol: format!("pkg{}/mod_{index:05}.py::caller_{index}", index % 40),
            target_symbol: format!(
                "pkg{}/mod_{:05}.py::callee_{}",
                (index + 1) % 40,
                index + 1,
                index + 1
            ),
            edge_kind: EdgeKind::Calls,
            confidence: Confidence::DETERMINISTIC,
            resolution: None,
            details: None,
        })
        .collect()
}

/// One module with a long call chain plus a caller module, so the store holds
/// thousands of edges from two `extract_file` calls rather than from thousands
/// of them. The fan-out cost under test is per *call*, not per edge walked.
fn chain_store(links: usize) -> (Store, Vec<String>) {
    let mut core = String::new();
    for index in 0..links {
        core.push_str(&format!(
            "def link_{index:05}(rows):\n    return link_{:05}(rows)\n\n\n",
            index + 1
        ));
    }
    core.push_str(&format!("def link_{links:05}(rows):\n    return rows\n"));

    let mut callers = String::from("from core import link_00000\n\n\n");
    for index in 0..8 {
        callers.push_str(&format!(
            "def entry_{index}(rows):\n    return link_00000(rows)\n\n\n"
        ));
    }

    let extractions = vec![
        extract_file("core.py", &core),
        extract_file("callers.py", &callers),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .expect("generation writes");

    let targets = (0..8)
        .map(|index| format!("core.py::link_{:05}", index * 11))
        .collect();
    (store, targets)
}

// ---------------------------------------------------------------- the test

#[test]
fn a_query_allocates_for_its_answer_not_for_the_whole_generation() {
    let edges = synthetic_edges(EDGES);

    // 1. Resolving a path-shaped traversal start.
    //
    // The target is one real file out of the 50,000, so the answer is a single
    // edge. Before the fix this allocated three `String`s per edge scanned —
    // 150,000 of them — to produce it.
    let target = edges[0].target_file.clone();
    let (allocations, starts) =
        allocations_during(|| devmap_query::traversal_starts(&edges, &target, true));
    assert!(
        !starts.is_empty(),
        "the fixture's own file must resolve as a start, or this measures a miss"
    );
    let path_budget = EDGES / 8;
    assert!(
        allocations < path_budget,
        "resolving a path-shaped traversal start allocated {allocations} times over \
         {EDGES} edges for {} start(s) — a per-edge allocation, not a per-answer one \
         (budget {path_budget})",
        starts.len()
    );

    // 2. Mapping the walk's edge identities back onto the generation.
    //
    // Bounded to 100 nodes, so the answer is ~99 edges out of 50,000. Before
    // the fix the scan built and dropped three `String`s for each of the
    // 50,000.
    let start_symbols: Vec<String> = devmap_query::traversal_starts(&edges, &target, true)
        .into_iter()
        .map(|(symbol, _)| symbol)
        .collect();
    let walk = traverse_graph(
        &start_symbols,
        &edges,
        &TraversalOptions {
            max_depth: 3,
            max_nodes: 100,
            reverse: true,
        },
    );
    let (allocations, kept) =
        allocations_during(|| devmap_query::traversed_resolution_edges(&walk, &edges, 0.0));
    assert!(
        !kept.is_empty(),
        "the walk must have found edges to map back, or this measures an empty scan"
    );
    let select_budget = EDGES / 8;
    assert!(
        allocations < select_budget,
        "selecting {} traversed edge(s) out of {EDGES} allocated {allocations} times — \
         a per-edge allocation, not a per-answer one (budget {select_budget})",
        kept.len()
    );

    // 3. Neither the composed fan-out nor the calls it composes read the edge
    //    table at all.
    //
    // This began as a *relative* assertion — composed against separate — because
    // the composition was the only side that had been taught to hoist the edge
    // load, and `impact`/`trace` still paid for the whole generation each. The
    // store now holds one adjacency index per generation, so both sides are
    // bounded by their answers and the ratio between them no longer measures
    // anything: measured here at 638 composed against 629 separate over a
    // 4,018-edge generation, where the relative form needed separate to be more
    // than twice composed.
    //
    // The absolute bound is what survives, and it is the stronger claim: eight
    // targets, two directions each, must allocate an order of magnitude below
    // the generation's edge count — a per-answer cost, not a per-generation
    // one. The relative half is kept as a no-regression floor: composing may
    // not cost *more* than issuing the same queries separately, which is what
    // a composition that re-read per sub-query would do.
    let (store, targets) = chain_store(2_000);
    let engine = StoreQueryEngine::new(&store);
    let request = |target: &String, depth: usize| Request {
        query: target.clone(),
        token_budget: 2_000,
        min_confidence: 0.0,
        max_depth: depth,
    };
    // Warm the store's edge cache first: the very first read of a generation
    // pays for SQL that neither side should be charged for.
    engine
        .neighbors(&targets, 2_000, 0.0, 1)
        .expect("neighbors answers");

    let (composed_allocations, composed) = allocations_during(|| {
        engine
            .neighbors(&targets, 2_000, 0.0, 1)
            .expect("neighbors")
    });
    let (separate_allocations, _) = allocations_during(|| {
        for target in &targets {
            std::hint::black_box(engine.impact(request(target, 1)).expect("impact"));
            std::hint::black_box(engine.trace(request(target, 1)).expect("trace"));
        }
    });
    assert_eq!(
        composed.len(),
        targets.len(),
        "the composition must answer for every target"
    );
    assert!(
        composed.iter().any(|entry| entry.callers.shown > 0),
        "the fixture must produce real callers, or both sides measure empty answers"
    );
    let generation_edges = store.latest_edges(0.0).expect("edges").len();
    assert!(
        generation_edges > 2_000,
        "the fixture must hold a generation large enough for a per-edge cost to \
         show, got {generation_edges} edge(s)"
    );
    let fan_out_budget = generation_edges / 2;
    assert!(
        composed_allocations < fan_out_budget,
        "neighbors over {} targets allocated {composed_allocations} times against a \
         {generation_edges}-edge generation — a per-generation cost, not a \
         per-answer one (budget {fan_out_budget})",
        targets.len()
    );
    assert!(
        separate_allocations < fan_out_budget,
        "the same queries issued one at a time allocated {separate_allocations} \
         times against a {generation_edges}-edge generation — `impact` and `trace` \
         are back to materialising the generation per call (budget {fan_out_budget})"
    );
    // No-regression floor, with the slack an allocator's bucket growth needs.
    // Composing must not cost more than the calls it replaces; it is allowed to
    // cost the same, which is what it now does.
    assert!(
        composed_allocations <= separate_allocations + separate_allocations / 4,
        "neighbors over {} targets allocated {composed_allocations} against \
         {separate_allocations} for the same queries issued one at a time — the \
         composition costs more than what it composes",
        targets.len()
    );
}
