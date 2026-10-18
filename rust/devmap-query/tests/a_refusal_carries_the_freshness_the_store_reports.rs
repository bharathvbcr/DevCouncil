//! A traversal that refuses still says what the store says about freshness.
//!
//! `unavailable_response` stamped "no persisted generation is available to
//! verify against the working tree" on every refusal, and `traverse` and
//! `trace_between` returned their refusals without routing them through
//! `finish`. So `impact` of an unknown name came back with that sentence next
//! to an `unresolved_namesakes.generation_id` for the generation it had just
//! read, and a depth-exhausted `trace_between` said the same where the found
//! arm carried the store's verdict.
//!
//! The sentence belongs to a store with no generation, and only to that.

use devmap_extract::extract_file;
use devmap_query::{QueryEngine, Request, Rung, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const NO_GENERATION: &str = "no persisted generation";

const CHAIN: &str = "def a():\n    return b()\n\n\ndef b():\n    return c()\n\n\ndef c():\n    return d()\n\n\ndef d():\n    return 1\n";

fn extractions() -> Vec<devmap_extract::model::Extraction> {
    vec![extract_file("chain.py", CHAIN)]
}

fn store() -> Store {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn req(query: &str, max_depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth,
    }
}

fn pair(from: &str, to: &str, max_depth: usize) -> Request<(String, String)> {
    Request {
        query: (from.to_string(), to.to_string()),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth,
    }
}

fn reason(freshness: &devmap_query::SourceFreshness) -> String {
    freshness.reason.clone().unwrap_or_default()
}

#[test]
fn impact_of_an_unknown_name_does_not_deny_the_generation_it_read() {
    let store = store();
    assert!(store.latest_generation_id().unwrap().unwrap() > 0);
    let answer = StoreQueryEngine::new(&store)
        .impact(req("does_not_exist_probe", 3))
        .unwrap();
    assert!(
        reason(&answer.source_freshness)
            .to_lowercase()
            .contains("not checked"),
        "an empty freshness cache says the check did not run: {:?}",
        answer.source_freshness
    );
    assert!(
        !reason(&answer.source_freshness).contains(NO_GENERATION),
        "a generation was read: {:?}",
        answer.source_freshness
    );
    if let Some(namesakes) = &answer.unresolved_namesakes {
        assert!(namesakes.generation_id > 0);
    }
}

#[test]
fn a_walk_toward_callees_that_refuses_carries_the_stores_freshness() {
    let store = store();
    let answer = StoreQueryEngine::new(&store)
        .trace(req("does_not_exist_probe", 3))
        .unwrap();
    assert!(!reason(&answer.source_freshness).contains(NO_GENERATION));
}

#[test]
fn a_min_rung_impact_refusal_carries_the_stores_freshness() {
    let store = store();
    let answer = StoreQueryEngine::new(&store)
        .impact_at_rung(req("does_not_exist_probe", 3), Some(Rung::Deterministic))
        .unwrap();
    assert!(!reason(&answer.source_freshness).contains(NO_GENERATION));
}

#[test]
fn layered_impact_refusal_carries_the_stores_freshness_on_both_halves() {
    let store = store();
    let answer = StoreQueryEngine::new(&store)
        .impact_layered(req("does_not_exist_probe", 3))
        .unwrap();
    assert!(!reason(&answer.edges.source_freshness).contains(NO_GENERATION));
    assert!(!reason(&answer.blast_radius.layers.source_freshness).contains(NO_GENERATION));
}

#[test]
fn a_depth_exhausted_trace_keeps_its_reason_and_the_stores_freshness() {
    let store = store();
    let engine = StoreQueryEngine::new(&store);
    let answer = engine.trace_between(pair("a", "d", 1)).unwrap();
    let resolution = format!("{:?}", answer.resolution);
    assert!(
        resolution.contains("stopped at depth"),
        "the stopped-at-depth reason survives: {resolution}"
    );
    assert!(
        !reason(&answer.source_freshness).contains(NO_GENERATION),
        "{:?}",
        answer.source_freshness
    );
    // Same attach as a found path.
    let found = engine.trace_between(pair("a", "d", 5)).unwrap();
    assert_eq!(
        found.source_freshness, answer.source_freshness,
        "found and exhausted arms attach the same verdict"
    );
}

#[test]
fn a_trace_with_no_path_carries_the_stores_freshness() {
    let store = store();
    let answer = StoreQueryEngine::new(&store)
        .trace_between(pair("d", "a", 5))
        .unwrap();
    let resolution = format!("{:?}", answer.resolution);
    assert!(resolution.contains("no indexed path"), "{resolution}");
    assert!(!reason(&answer.source_freshness).contains(NO_GENERATION));
}

#[test]
fn a_store_with_no_generation_still_says_so() {
    let store = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&store);
    let impact = engine.impact(req("anything", 3)).unwrap();
    assert!(reason(&impact.source_freshness).contains(NO_GENERATION));
    let trace = engine.trace_between(pair("a", "b", 3)).unwrap();
    assert!(reason(&trace.source_freshness).contains(NO_GENERATION));
    let layered = engine.impact_layered(req("anything", 3)).unwrap();
    assert!(reason(&layered.edges.source_freshness).contains(NO_GENERATION));
}

#[test]
fn the_in_memory_engine_has_no_generation_to_deny() {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let engine = QueryEngine::new(&extractions, &resolution);
    let impact = engine.impact(req("does_not_exist_probe", 3));
    assert!(!reason(&impact.source_freshness).contains(NO_GENERATION));
    let trace = engine.trace(req("does_not_exist_probe", 3));
    assert!(!reason(&trace.source_freshness).contains(NO_GENERATION));
}
