//! Timing probe for the refusal path: 100 refusals of each kind over a
//! repository-sized store, min of 5. Run with `--release -- --ignored --nocapture`.
//! Uses only API that exists both before and after ft-b79f84e2, so the same file
//! can be dropped into the baseline commit.

use std::time::Instant;

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn req(query: &str) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth: 3,
    }
}

fn min_of_5(mut run: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        for _ in 0..100 {
            run();
        }
        best = best.min(started.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

#[test]
#[ignore]
fn p5_time_100_refusals_on_a_repo_sized_store() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap();
    let (files, _report) = extract_tree_with_report(&root).unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &files,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store.status("t").unwrap();
    let engine = StoreQueryEngine::new(&store);
    // Warm the per-generation index once so the loop measures the refusal.
    engine.impact(req("zz_no_such_symbol_anywhere")).unwrap();
    let impact = min_of_5(|| {
        engine.impact(req("zz_no_such_symbol_anywhere")).unwrap();
    });
    let trace = min_of_5(|| {
        engine.trace(req("zz_no_such_symbol_anywhere")).unwrap();
    });
    let between = min_of_5(|| {
        engine
            .trace_between(Request {
                query: ("zz_no_a".to_string(), "zz_no_b".to_string()),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 3,
            })
            .unwrap();
    });
    let layered = min_of_5(|| {
        engine.impact_layered(req("zz_no_such_symbol_anywhere")).unwrap();
    });
    let finish_only = min_of_5(|| {
        let _ = store.query_source_freshness();
    });
    eprintln!(
        "files={} impact={impact:.2}ms trace={trace:.2}ms trace_between={between:.2}ms impact_layered={layered:.2}ms (per 100, min of 5); query_source_freshness x100 = {finish_only:.3}ms",
        files.len()
    );
}
