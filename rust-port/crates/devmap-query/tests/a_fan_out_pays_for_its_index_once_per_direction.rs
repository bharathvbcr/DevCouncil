//! A fan-out may walk per target; it may not *index* per target.
//!
//! `neighbors` asks for both directions of up to 16 targets, and `explore`
//! does the same per definition. Each of those walks began by building an
//! adjacency map over the whole generation, so N targets rebuilt the same two
//! maps 2N times over an edge slice that does not change between them. There
//! are only ever two directions, so there are only ever two indexes.
//!
//! `query_work_is_bounded_by_the_answer.rs` already measures the *edge load*
//! hoist that came before this one, by comparing the composition against the
//! same queries issued separately. That comparison cannot see this defect:
//! with the index rebuilt per target, the composed call is still cheaper than
//! the separate calls by the whole edge read, so it stays green either way.
//! This file therefore compares the composition against *itself* at two
//! target counts, which is the only shape that isolates a per-target cost.
//!
//! Counting allocations rather than time, for the reason the sibling file
//! gives: the counts are deterministic, and a timing assertion turns a loaded
//! machine into a red build. The allocator is process-wide, so this file holds
//! exactly one `#[test]` — a second would run concurrently in the same binary
//! and each would count the other's work. That is also why this is a separate
//! binary rather than another case in the sibling file.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use devmap_extract::extract_file;
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

// ------------------------------------------------------- counting allocator

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

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

// ------------------------------------------------------------------ fixture

/// One long call chain plus a caller module, so the store holds thousands of
/// edges from two `extract_file` calls. The cost under test is per *target*,
/// not per edge walked, so the walks themselves are kept shallow.
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

    let targets = (0..16)
        .map(|index| format!("core.py::link_{:05}", index * 37))
        .collect();
    (store, targets)
}

// ---------------------------------------------------------------- the test

#[test]
fn a_fan_out_does_not_pay_for_an_index_per_target() {
    let (store, targets) = chain_store(6_000);
    let engine = StoreQueryEngine::new(&store);
    let one = &targets[..1];

    // Warm the store's edge cache: the first read of a generation pays for SQL
    // that neither measurement should be charged for.
    engine
        .neighbors(&targets, 2_000, 0.0, 1)
        .expect("neighbors");

    let (one_target, one_answer) =
        allocations_during(|| engine.neighbors(one, 2_000, 0.0, 1).expect("neighbors"));
    let (many_targets, many_answer) = allocations_during(|| {
        engine
            .neighbors(&targets, 2_000, 0.0, 1)
            .expect("neighbors")
    });

    assert_eq!(one_answer.len(), 1, "the one-target call must answer");
    assert_eq!(
        many_answer.len(),
        targets.len(),
        "the composition must answer for every target"
    );
    // Both sides must do real work, or this compares two empty answers.
    assert!(
        many_answer.iter().any(|entry| entry.callers.shown > 0),
        "the fixture must produce real callers"
    );

    // Sixteen targets legitimately cost more than one — sixteen walks, sixteen
    // budgeted answers, sixteen sets of reported edges — and that marginal
    // cost is the answer, not the graph. What must not scale is the index.
    //
    // Measured on this fixture, with the hoist reverted so the index is
    // rebuilt per target and then restored, everything else identical:
    //
    //     index per target     299,720 for 16 targets vs 88,310 for one — 3.4x
    //     index per direction   75,160 for 16 targets vs 74,275 for one — 1.0x
    //
    // The same answer costs 4x the allocations when the index is not shared,
    // and the per-target growth is the entire difference. Two sits between
    // those worlds with room on both sides rather than being tuned to either.
    assert!(
        many_targets < one_target * 2,
        "neighbors over {} targets allocated {many_targets} against {one_target} for a \
         single target — a {:.1}x ratio. The adjacency index is being rebuilt per \
         target instead of once per direction.",
        targets.len(),
        many_targets as f64 / one_target as f64
    );
}
