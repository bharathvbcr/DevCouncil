//! `dead_symbols` may rank the whole corpus; it may not *materialise* it.
//!
//! The surface reads a generation's dead-symbol rows and the budgeter keeps a
//! few dozen. It used to read every row and then discard the exempt ones in
//! Rust — measured at 80,000 rows read to show 66 on the benchmark corpus, and
//! on this repository 6,976 of the 7,176 rows read were exempt and dropped on
//! arrival. The cost tracked the corpus; the answer never did.
//!
//! What makes the bound safe rather than a quiet truncation is the count that
//! travels with it. `Response` carries `shown + hidden == total` and clients
//! enforce it, so a bounded read that also shrank `total` would turn "66 of
//! 80,000" into "66 of 66" — a capped list reporting itself as the whole
//! truth. Both halves are asserted here: the work must not scale, and the
//! denominator must.
//!
//! Counting allocations rather than time, and one `#[test]` per binary, for
//! the reasons `query_work_is_bounded_by_the_answer.rs` gives.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use devmap_extract::extract_file;
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

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

/// A generation holding `count` uncalled functions, so `count` dead symbols.
fn dead_store(count: usize) -> Store {
    let mut source = String::new();
    for index in 0..count {
        source.push_str(&format!(
            "def unused_{index:05}(rows):\n    return rows\n\n\n"
        ));
    }
    let extractions = vec![extract_file("orphans.py", &source)];
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
    store
}

/// 20 rows at `DEAD_SYMBOL_TOKENS` (30) each — far below either corpus.
const BUDGET: u32 = 600;

#[test]
fn a_delete_this_list_reads_for_its_answer_not_for_the_corpus() {
    let small = dead_store(200);
    let large = dead_store(2_000);
    let small_engine = StoreQueryEngine::new(&small);
    let large_engine = StoreQueryEngine::new(&large);

    // Untimed first calls, so neither measurement pays a cache warm-up.
    small_engine.dead_symbols(BUDGET).expect("dead_symbols");
    large_engine.dead_symbols(BUDGET).expect("dead_symbols");

    let (small_allocations, small_answer) =
        allocations_during(|| small_engine.dead_symbols(BUDGET).expect("dead_symbols"));
    let (large_allocations, large_answer) =
        allocations_during(|| large_engine.dead_symbols(BUDGET).expect("dead_symbols"));

    // The corpora must actually differ by an order of magnitude, or the
    // comparison below measures nothing.
    assert!(
        small_answer.total >= 100 && large_answer.total >= 1_000,
        "the fixtures must produce dead symbols in bulk: {} and {}",
        small_answer.total,
        large_answer.total
    );
    assert!(
        large_answer.total >= small_answer.total * 5,
        "the large corpus must dwarf the small one: {} vs {}",
        large_answer.total,
        small_answer.total
    );

    // The denominator survives the bound. This is the half that makes the
    // bound honest rather than a silent truncation.
    assert_eq!(
        large_answer.shown + large_answer.hidden,
        large_answer.total,
        "shown + hidden must equal total, or a client's own check fails"
    );
    assert!(
        large_answer.truncated && large_answer.hidden > 0,
        "a list cut at {} of {} must say it was cut",
        large_answer.shown,
        large_answer.total
    );
    // Same budget, same page: the answer is the budget's size, not the corpus'.
    assert_eq!(
        small_answer.shown, large_answer.shown,
        "the same budget over two corpora must seat the same number of rows"
    );

    // And the work does not follow the corpus. Ten times the dead symbols for
    // the same twenty rows: unbounded, that is ten times the reading; bounded,
    // it is the same read twice. Two sits between those without being tuned to
    // either.
    assert!(
        large_allocations < small_allocations * 2,
        "dead_symbols over {} dead symbols allocated {large_allocations} against \
         {small_allocations} over {} — a {:.1}x ratio for the same {} rows shown. \
         The read is still proportional to the corpus.",
        large_answer.total,
        small_answer.total,
        large_allocations as f64 / small_allocations as f64,
        large_answer.shown
    );
}
