//! Answers both engines give through the crate's public API alone.
//!
//! Moved out of `engine.rs`'s inline `tests` module, whose remaining cases read
//! private budget, trace and source-span internals and so stay a child of
//! `engine` in `src/engine/tests/tests.rs`. Declared `required-features =
//! ["parse"]`, as that module was gated: every case extracts real source.

use devmap_extract::extract_file;
use devmap_query::model::{Request, ResolutionAvailability};
use devmap_query::{QueryEngine, StoreQueryEngine};
use devmap_resolve::Resolver;

#[test]
fn search_reports_shown_and_total_when_truncated() {
    let mut src = String::new();
    for i in 0..50 {
        src.push_str(&format!("def fn_{i}():\n    return {i}\n"));
    }
    let ext = extract_file("mod.py", &src);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext)).unwrap();
    let exts = [ext];
    let engine = QueryEngine::new(&exts, &resolution);
    let resp = engine.search(Request {
        query: "fn_".into(),
        token_budget: 80,
        min_confidence: 0.0,
        max_depth: 1,
    });
    assert!(resp.total > resp.shown);
    assert!(resp.truncated);
    assert_eq!(resp.shown, resp.items.len() as u32);
}

#[test]
fn persisted_engine_answers_without_reextracting_sources() {
    use devmap_analyze::analyze;
    use devmap_store::Store;

    let target = extract_file("missing/target.py", "def target():\n    return 1\n");
    let caller = extract_file(
        "missing/caller.py",
        "from target import target\n\ndef caller():\n    return target()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[target.clone(), caller.clone()]);
    let resolution = resolver
        .resolve_all(&[target.clone(), caller.clone()])
        .unwrap();
    let analysis = analyze(&[target.clone(), caller.clone()], &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&[target, caller], &resolution, &analysis)
        .unwrap();

    let engine = StoreQueryEngine::new(&store);
    let search = engine
        .search(Request {
            query: "target".into(),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    assert!(search
        .items
        .iter()
        .any(|item| item.file_path == "missing/target.py"));
    let target_hit = search
        .items
        .iter()
        .find(|item| item.file_path == "missing/target.py")
        .expect("persisted target hit");
    assert!(target_hit.source_span.is_empty());
    assert!(target_hit.source_unavailable_reason.is_some());

    let deps = engine
        .dependencies(Request {
            query: "missing/caller.py".into(),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    assert!(matches!(deps.resolution, ResolutionAvailability::Available));
    assert!(deps.items.iter().any(|edge| {
        edge.source_file == "missing/caller.py" && edge.target_file == "missing/target.py"
    }));
}

#[test]
fn persisted_engine_preserves_unavailable_and_dead_states() {
    use devmap_analyze::analyze;
    use devmap_store::Store;

    let ext = extract_file("dead.py", "def abandoned():\n    return 1\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext)).unwrap();
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    let engine = StoreQueryEngine::new(&store);
    let missing = engine
        .dependencies(Request {
            query: "absent.py".into(),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    assert!(matches!(
        missing.resolution,
        ResolutionAvailability::Unavailable { .. }
    ));
    let dead = engine.dead_symbols(2_000).unwrap();
    assert!(dead
        .items
        .iter()
        .any(|item| item.symbol_name == "abandoned"));
}
