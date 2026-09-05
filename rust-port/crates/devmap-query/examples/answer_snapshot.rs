//! One line of JSON per (surface, target), for differential comparison.
//!
//! ```sh
//! cargo run --release -p devmap-query --example answer_snapshot -- <devmap.sqlite> [targets-file]
//! ```
//!
//! Exists so that a rewrite of how the traversal reaches its edges can be held
//! to *byte-identical answers* rather than to a reviewer's reading of the diff.
//! Run it against the old binary, run it against the new one, `diff` the two
//! files: anything that moved is a behaviour change, named.
//!
//! With no target file it derives its own target list from the store — the
//! busiest callees, the leaves, some file paths, and some absent names — so
//! the comparison covers callers-heavy, leaf, absent, path-shaped and
//! symbol-shaped queries without a hand-written fixture per corpus.

use std::collections::BTreeSet;

use devmap_query::{Request, StoreQueryEngine};

fn targets_from_store(store: &devmap_store::Store, wanted: usize) -> Vec<String> {
    let mut targets: BTreeSet<String> = BTreeSet::new();
    let edges = store.latest_edges(0.0).expect("edges");

    // Callers-heavy: the symbols the most edges point at.
    let mut inbound: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut outbound: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for edge in &edges {
        *inbound.entry(edge.target_symbol.as_str()).or_default() += 1;
        *outbound.entry(edge.source_symbol.as_str()).or_default() += 1;
    }
    let mut by_inbound: Vec<(&str, usize)> = inbound.into_iter().collect();
    by_inbound.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    for (symbol, _) in by_inbound.iter().take(wanted / 4) {
        targets.insert((*symbol).to_string());
    }
    // Leaves: named by an edge but with nothing leaving them.
    let mut leaves: Vec<&str> = by_inbound
        .iter()
        .map(|(symbol, _)| *symbol)
        .filter(|symbol| !outbound.contains_key(symbol))
        .collect();
    leaves.sort_unstable();
    for symbol in leaves.iter().take(wanted / 4) {
        targets.insert((*symbol).to_string());
    }
    // File-path shaped.
    let mut files: Vec<&str> = edges.iter().map(|edge| edge.source_file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let stride = (files.len() / (wanted / 4).max(1)).max(1);
    for file in files.iter().step_by(stride).take(wanted / 4) {
        targets.insert((*file).to_string());
    }
    // Bare symbol tails, and names nothing can match.
    for (symbol, _) in by_inbound.iter().take(wanted) {
        if let Some(tail) = symbol.rsplit("::").next() {
            targets.insert(tail.to_string());
        }
        if targets.len() >= wanted {
            break;
        }
    }
    for absent in [
        "",
        "   ",
        "no_such_symbol_anywhere_42",
        "no/such/path.go",
        "no_such.py::missing",
        "::",
        "a::b::c",
    ] {
        targets.insert(absent.to_string());
    }
    targets.into_iter().collect()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: answer_snapshot <db>");
    let store = devmap_store::Store::open_existing(&path)
        .expect("open")
        .expect("a store must exist at that path");
    let targets = match std::env::args().nth(2) {
        Some(file) => std::fs::read_to_string(file)
            .expect("target list")
            .lines()
            .map(str::to_string)
            .collect(),
        None => targets_from_store(&store, 240),
    };

    let engine = StoreQueryEngine::new(&store);
    for target in &targets {
        for confidence in [0.0f32, 0.5, 0.95] {
            for depth in [1usize, 5] {
                let request = |query: &str| Request {
                    query: query.to_string(),
                    token_budget: 10_000,
                    min_confidence: confidence,
                    max_depth: depth,
                };
                let impact = engine.impact(request(target));
                let trace = engine.trace(request(target));
                let deps = engine.dependencies(request(target));
                let affected =
                    engine.affected_tests(std::slice::from_ref(target), 10_000, confidence, depth);
                let explore = engine.explore(target, 5, 10_000, confidence, depth);
                let line = serde_json::json!({
                    "target": target,
                    "min_confidence": confidence,
                    "max_depth": depth,
                    "impact": impact.as_ref().map_err(|e| e.to_string()).map(|r| serde_json::to_value(r).unwrap()),
                    "trace": trace.as_ref().map_err(|e| e.to_string()).map(|r| serde_json::to_value(r).unwrap()),
                    "dependencies": deps.as_ref().map_err(|e| e.to_string()).map(|r| serde_json::to_value(r).unwrap()),
                    "affected": affected.as_ref().map_err(|e| e.to_string()).map(|r| serde_json::to_value(r).unwrap()),
                    "explore": explore.as_ref().map_err(|e| e.to_string()).map(|r| serde_json::to_value(r).unwrap()),
                });
                println!("{line}");
            }
        }
    }
    eprintln!("{} targets x 6 request shapes", targets.len());
}
