//! How much memory a bounded graph walk is allowed to cost.
//!
//! **Its own test binary, deliberately.** The measurement is a process-global
//! allocation counter, and `cargo test` runs the tests inside one binary on
//! several threads — so a sibling test building a 160,000-edge fixture lands in
//! this test's total and makes the number meaningless. Measured that way once,
//! it read 592 bytes/edge where the real figure was 105. A file with a single
//! test is a process with a single test.
//!
//! **Why allocation and not wall time.** Wall time on a machine shared with
//! other builds says as much about the other builds as about this code. "How
//! many bytes did answering a 64-node question cost" is the defect stated
//! directly, and it is reproducible.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use devmap_analyze::traversal::{traverse_graph, TraversalOptions};
use devmap_extract::model::{Confidence, EdgeKind};
use devmap_resolve::model::ResolvedEdge;

/// Bytes handed out by the allocator since this binary started.
///
/// `std` only — no allocator crate enters the dependency tree for this.
/// Deallocation is not subtracted: the question is how much the walk *asks
/// for*, and a walk that allocates and frees a copy of the graph has still
/// copied the graph.
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout)
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let grown = System.realloc(pointer, layout, new_size);
        if !grown.is_null() {
            ALLOCATED.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        }
        grown
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Bytes a walk may allocate per edge of the graph it was handed.
///
/// Measured A/B in a release build, 64-node cap, same fixture:
///
/// | edges   | cloning index | borrowing index |
/// |---------|---------------|-----------------|
/// | 50,064  | 661.7 B/edge, 16.2 ms | 104.8 B/edge, 7.0 ms |
/// | 500,064 | 664.5 B/edge, 205.9 ms | 104.6 B/edge, 71.7 ms |
///
/// The remaining 105 B/edge is the `BTreeMap` node and the per-key `Vec`, not a
/// copy of any edge. 200 sits between the two regimes with a factor of two of
/// headroom on each side, so this fails on a return to cloning and does not
/// fail on allocator or container churn.
const MAX_BYTES_PER_INPUT_EDGE: usize = 200;

/// A 64-node walk over a 500k-edge graph, in a debug build, on a shared machine.
const WALK_BUDGET: Duration = Duration::from_secs(10);

fn edge(source: &str, target: &str) -> ResolvedEdge {
    ResolvedEdge {
        source_file: "a.go".to_string(),
        target_file: "b.go".to_string(),
        source_symbol: source.to_string(),
        target_symbol: target.to_string(),
        edge_kind: EdgeKind::Calls,
        confidence: Confidence::DETERMINISTIC,
        resolution: None,
        details: None,
        evidence: None,
    }
}

/// A bounded walk must not allocate a copy of the graph it walks.
///
/// `traverse_graph` built its adjacency index by **cloning every edge** — four
/// `String`s each — before `max_nodes` was consulted at all. On DevCouncil's
/// personal corpus that is ~944,000 deep clones, roughly 625 MB of allocation,
/// to answer a question capped at 1,000 nodes; and it is paid on every `impact`
/// and every `trace`.
///
/// What this does *not* assert is that the walk is cheaper than O(edges) in
/// time. It cannot be: a walk handed an unindexed slice must look at every edge
/// once to know what is adjacent to anything. That floor is a property of the
/// interface, not a defect, and pretending otherwise would put an unreachable
/// bar in a test file. Bytes copied has no such floor.
#[test]
fn a_capped_walk_does_not_copy_the_graph_it_walks() {
    fn walk(far_edges: usize) -> (usize, usize, Duration) {
        // A short chain the walk can follow, plus a wide region it can never
        // reach. Only the chain is reachable, so a 64-node walk reports 64
        // nodes however wide the unreachable part is — and any allocation that
        // scales with the unreachable part is a copy of the graph rather than a
        // record of the answer.
        let mut graph: Vec<ResolvedEdge> = (0..64)
            .map(|i| edge(&format!("n{i}"), &format!("n{}", i + 1)))
            .collect();
        graph.extend((0..far_edges).map(|i| edge(&format!("far{i}"), &format!("far{}", i + 1))));

        let before = ALLOCATED.load(Ordering::SeqCst);
        let started = Instant::now();
        let result = traverse_graph(
            &["n0".to_string()],
            &graph,
            &TraversalOptions {
                max_depth: 64,
                max_nodes: 64,
                reverse: false,
            },
        );
        let elapsed = started.elapsed();
        let allocated = ALLOCATED.load(Ordering::SeqCst).saturating_sub(before);
        assert_eq!(
            result.visited_nodes.len(),
            64,
            "the fixture must reach exactly its cap, or the measurement below \
             describes a different walk"
        );
        (graph.len(), allocated, elapsed)
    }

    walk(10_000); // warm the allocator's arenas and the code paths
    let measurements = [walk(50_000), walk(500_000)];

    for (edges, bytes, elapsed) in measurements {
        assert!(
            elapsed < WALK_BUDGET,
            "a 64-node walk over {edges} edges took {elapsed:?}"
        );
        let per_edge = bytes as f64 / edges as f64;
        assert!(
            per_edge < MAX_BYTES_PER_INPUT_EDGE as f64,
            "a 64-node walk over {edges} edges allocated {bytes} bytes \
             ({per_edge:.0} per input edge, budget {MAX_BYTES_PER_INPUT_EDGE}); \
             the adjacency index is copying the graph instead of borrowing it"
        );
    }

    // The per-edge cost must be flat, not merely small at one size: a term that
    // grows with graph size would still be a copy, just a cheaper one.
    let (small_edges, small_bytes, _) = measurements[0];
    let (large_edges, large_bytes, _) = measurements[1];
    let small_rate = small_bytes as f64 / small_edges as f64;
    let large_rate = large_bytes as f64 / large_edges as f64;
    assert!(
        large_rate < small_rate * 1.5,
        "per-edge allocation grew with the graph ({small_rate:.0} -> \
         {large_rate:.0} bytes/edge); the index cost is superlinear"
    );
}
