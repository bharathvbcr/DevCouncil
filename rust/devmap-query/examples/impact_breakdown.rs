//! Where an `impact` call's time goes.
//!
//! Measurement only, no assertions. Run:
//!
//! ```sh
//! cargo run --release -p devmap-query --example impact_breakdown -- <devmap.sqlite>
//! ```
//!
//! Point it at a store that has settled — a generation still being swept
//! measures the sweep. It refuses an empty store rather than reporting a fast
//! query over nothing.
//!
//! Recorded 2026-09-05 on a 14,989-node / 77,904-edge store: the SQL read is
//! 5.3 ms of a 33.6 ms call. The other 84% is `stored_edge_to_resolved`
//! converting every one of those 77,904 rows into an owned `ResolvedEdge`
//! before the traversal has looked at the target — see STATUS.md.

use std::time::Instant;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: impact_breakdown <db>");
    let store = devmap_store::Store::open_existing(&path)
        .expect("open")
        .expect("a store must exist at that path");
    let status = store.status(&path).expect("status");
    println!(
        "store: nodes={} edges={} generation={:?}",
        status.node_count, status.edge_count, status.latest_generation
    );
    assert!(status.edge_count > 0, "refusing to measure an empty store");

    let mut read = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let rows = store.latest_edges(0.0).expect("edges");
        read.push(t.elapsed());
        std::hint::black_box(&rows);
    }
    read.sort();

    let engine = devmap_query::StoreQueryEngine::new(&store);
    let mut whole = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let response = engine
            .impact(devmap_query::Request {
                query: "helper".to_string(),
                token_budget: 10_000,
                min_confidence: 0.0,
                max_depth: 5,
            })
            .expect("impact");
        whole.push(t.elapsed());
        std::hint::black_box(&response);
    }
    whole.sort();

    println!(
        "latest_edges (SQL read only) p50 = {:?}",
        read[read.len() / 2]
    );
    println!(
        "impact (read + convert + walk) p50 = {:?}",
        whole[whole.len() / 2]
    );
    println!(
        "=> the read is {:.0}% of the whole call",
        100.0 * read[read.len() / 2].as_secs_f64() / whole[whole.len() / 2].as_secs_f64()
    );
}
