//! Adversarial coverage for `devmap ask`.
//!
//! Locks the behaviours section 3 of the graft plan requires: empty query,
//! a query that matches every name, a call-graph cycle, cancel mid-iteration,
//! and a seed set whose only call edges are speculative (empty at the default
//! floor, with an explicit withheld line).

use std::collections::HashSet;
use std::sync::Arc;

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, Extraction};
use devmap_query::ask::{call_adjacency, confidence_withheld_reason};
use devmap_query::{Cancel, StoreQueryEngine, ASK_DEFAULT_MIN_CONFIDENCE};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    store_of_extractions(extractions)
}

fn store_of_extractions(extractions: Vec<Extraction>) -> Store {
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

fn ask(
    store: &Store,
    query: &str,
    min_confidence: f32,
) -> devmap_query::Response<devmap_query::SymbolHit> {
    StoreQueryEngine::new(store)
        .ask(query, 10_000, min_confidence)
        .unwrap()
}

/// Two `cache_*` seeds that only call an ambiguous `shared` name.
///
/// The resolver records speculative Calls edges from each seed to both
/// `shared` definitions. At the deterministic floor those edges are excluded,
/// so ask must return empty with the withheld line rather than the TF-IDF
/// seeds.
fn speculative_seed_store() -> Store {
    store_of(&[
        ("a.py", "def cache_alpha():\n    shared()\n"),
        ("b.py", "def cache_beta():\n    shared()\n"),
        ("c1.py", "def shared():\n    return 1\n"),
        ("c2.py", "def shared():\n    return 2\n"),
    ])
}

#[test]
fn an_empty_query_returns_nothing() {
    let store = store_of(&[("a.py", "def cache_llm_reply():\n    return 1\n")]);
    let response = ask(&store, "", ASK_DEFAULT_MIN_CONFIDENCE);
    assert_eq!(response.total, 0, "{response:?}");
    assert!(response.items.is_empty());
    assert!(!response.truncated);
}

#[test]
fn no_shared_terms_is_empty_not_the_whole_corpus() {
    let store = store_of(&[
        ("a.py", "def alpha():\n    return 1\n"),
        ("b.py", "def beta():\n    return 2\n"),
    ]);
    let response = ask(&store, "zebra quokka", ASK_DEFAULT_MIN_CONFIDENCE);
    assert_eq!(response.total, 0, "{response:?}");
    assert!(response.items.is_empty());
    let gap = response.walk_incomplete.as_deref().unwrap_or("");
    assert!(
        gap.contains("no name or docstring shared a term"),
        "empty ask must say why, got {gap:?}"
    );
}

#[test]
fn a_query_that_matches_every_name_still_budgets() {
    // Every symbol name carries `handler`, so TF-IDF seeds the whole corpus.
    // A tiny budget must truncate rather than materialise every hit.
    let files: Vec<(String, String)> = (0..40)
        .map(|i| {
            (
                format!("m{i}.py"),
                format!("def handler_{i}():\n    return {i}\n"),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    let store = store_of(&borrowed);
    let response = StoreQueryEngine::new(&store)
        .ask("handler", 80, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert!(
        response.total >= 40,
        "every handler_* must seed: total={}",
        response.total
    );
    assert!(
        response.truncated,
        "a budget that cannot show every seed must set truncated: {response:?}"
    );
    assert!(response.hidden > 0, "{response:?}");
}

#[test]
fn a_call_cycle_does_not_hang_the_rerank() {
    let store = store_of(&[(
        "cycle.py",
        "def ping():\n    return pong()\n\ndef pong():\n    return ping()\n",
    )]);
    let response = ask(&store, "ping pong", ASK_DEFAULT_MIN_CONFIDENCE);
    assert!(
        response.total >= 2,
        "both ends of the cycle should seed: {response:?}"
    );
}

#[test]
fn cancel_mid_iteration_refuses_instead_of_answering() {
    let store = store_of(&[(
        "a.py",
        "def cache_write():\n    return 1\n\ndef cache_read():\n    return cache_write()\n",
    )]);
    let cancel = Cancel::new();
    cancel.cancel();
    let error = StoreQueryEngine::new(&store)
        .with_cancel(cancel)
        .ask("cache", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .expect_err("a cancelled ask must refuse");
    let message = format!("{error:#}").to_lowercase();
    assert!(
        message.contains("cancel"),
        "refusal must name cancellation, got {message}"
    );
}

#[test]
fn speculative_only_seeds_are_withheld_at_the_default_floor() {
    let store = speculative_seed_store();

    // Control: name TF-IDF still finds the seeds.
    let semantic = StoreQueryEngine::new(&store)
        .search_semantic("cache", 10_000)
        .unwrap();
    assert!(
        semantic.total >= 2,
        "control: name TF-IDF must seed cache_*: {semantic:?}"
    );

    // Precondition: the only call edges touching those seeds are below the
    // deterministic floor. If the resolver ever starts binding them, this
    // fixture stops proving the withheld path and must be rebuilt.
    let edges = store.generation_edges().unwrap().expect("edges");
    let seed_names: HashSet<&str> = ["a.py::cache_alpha", "b.py::cache_beta"]
        .into_iter()
        .collect();
    let mut any = false;
    let mut admitted = false;
    for id in 0..edges.len() as u32 {
        if edges.kind(id) != devmap_extract::model::EdgeKind::Calls {
            continue;
        }
        let touches = seed_names.contains(edges.source_symbol(id))
            || seed_names.contains(edges.target_symbol(id));
        if !touches {
            continue;
        }
        any = true;
        if edges.admits(id, ASK_DEFAULT_MIN_CONFIDENCE) {
            admitted = true;
        }
    }
    assert!(
        any && !admitted,
        "fixture must expose speculative-only edges on the seeds; any={any} admitted={admitted}"
    );

    let at_default = ask(&store, "cache", ASK_DEFAULT_MIN_CONFIDENCE);
    assert_eq!(
        at_default.total, 0,
        "default floor must withhold speculative-only seeds: {at_default:?}"
    );
    let gap = at_default.walk_incomplete.as_deref().unwrap_or("");
    assert!(
        gap.contains("withheld for confidence") || gap.contains(&confidence_withheld_reason()),
        "must say matches were withheld for confidence, not absent: {gap:?}"
    );

    // Lowering the floor re-admits the edges and returns the seeds.
    let lowered = ask(&store, "cache", Confidence::SPECULATIVE.0);
    assert!(
        lowered.total >= 2,
        "lowering min_confidence must return the seeds: {lowered:?}"
    );
}

#[test]
fn docstring_terms_seed_when_present() {
    let mut extraction = extract_file("doc.py", "def obscure_name():\n    return 1\n");
    for symbol in &mut extraction.symbols {
        if symbol.name == "obscure_name" {
            symbol.docstring = Some("evicts stale llm cache entries".into());
        }
    }
    let store = store_of_extractions(vec![extraction]);
    let by_doc = ask(&store, "llm cache eviction", ASK_DEFAULT_MIN_CONFIDENCE);
    assert!(
        by_doc.total >= 1,
        "docstring terms must seed when present: {by_doc:?}"
    );
    assert!(
        by_doc
            .items
            .iter()
            .any(|hit| hit.symbol_name == "obscure_name"),
        "{by_doc:?}"
    );

    // Names-only semantic search must stay names-only.
    let semantic = StoreQueryEngine::new(&store)
        .search_semantic("llm cache eviction", 10_000)
        .unwrap();
    assert_eq!(
        semantic.total, 0,
        "search_semantic must remain names-only; got {semantic:?}"
    );
}

#[test]
fn call_adjacency_excludes_edges_below_the_floor() {
    let index = devmap_store::GenerationEdges::build(
        Arc::new(vec![devmap_store::StoredEdge {
            source_file: "spec.py".into(),
            target_file: "spec.py".into(),
            source_symbol: "spec.py::cache_alpha".into(),
            target_symbol: "spec.py::cache_beta".into(),
            edge_kind: "Calls".into(),
            confidence: Confidence::SPECULATIVE.0,
            resolution: Some("AmbiguousGlobal".into()),
        }]),
        None,
    )
    .unwrap();
    let nodes = vec!["spec.py::cache_alpha".into(), "spec.py::cache_beta".into()];
    let seeds: HashSet<&str> = ["spec.py::cache_alpha", "spec.py::cache_beta"]
        .into_iter()
        .collect();
    let (_outbound, any, admitted) =
        call_adjacency(&index, &nodes, &seeds, ASK_DEFAULT_MIN_CONFIDENCE);
    assert!(any && !admitted);
    let (_outbound, any, admitted) =
        call_adjacency(&index, &nodes, &seeds, Confidence::SPECULATIVE.0);
    assert!(any && admitted);
}
