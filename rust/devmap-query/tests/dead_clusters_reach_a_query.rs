//! **R8.** The highest-recall pass in the analysis reached no query path.
//!
//! `dead_clusters.rs` is ~500 lines with five test files behind it, and it finds
//! the one thing a one-hop inbound-edge join structurally cannot: a subsystem
//! whose functions call each other has an inbound edge on *every* member, so
//! `dead_symbols` reports none of it at any confidence. That is the largest
//! recall hole the liveness pass has, and it was closed at build time and then
//! dropped on the floor.
//!
//! Its output reached exactly two artifacts — `manifest.rs` and
//! `code_graph.rs` — and no query surface at all:
//!
//! * `StoreQueryEngine::dead_symbols` returned `Response<DeadSymbolReport>` from
//!   `store.dead_page` and nothing else;
//! * `devmap dead` rendered rows and a truncation line;
//! * the IPC `Dead` response carried none, so `DevMapClient.dead_symbols` could
//!   not see them;
//! * the MCP envelope had no key for them;
//! * `dev graph dead` built its entries from `rust_dead["dead_code"]`;
//! * `CodeGraph` had no field, so pydantic dropped them at load.
//!
//! `grep dead_clusters src/devcouncil` returned nothing. The only Python reader
//! in the repository was a benchmark script.
//!
//! This file asserts the query half end to end: build a corpus containing an
//! abandoned cycle, persist it, and read it back through the surface an agent
//! actually calls.

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::Store;

/// Three functions that call only each other, reachable from nothing, plus one
/// ordinary uncalled symbol so both populations are present at once.
const CORPUS: &[(&str, &str)] = &[
    (
        "app/ring.py",
        "def _alpha():\n    return _beta()\n\n\n\
         def _beta():\n    return _gamma()\n\n\n\
         def _gamma():\n    return _alpha()\n",
    ),
    (
        "app/lonely.py",
        "def stranded():\n    return 1\n\n\ndef main():\n    return 2\n",
    ),
];

fn extractions() -> Vec<Extraction> {
    CORPUS
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

/// A store holding one generation of `CORPUS`.
fn persisted() -> Store {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    assert!(
        !analysis.dead_clusters.clusters.is_empty(),
        "the fixture must produce an abandoned cycle, or every assertion below \
         holds vacuously: {:?}",
        analysis.dead_clusters
    );
    let store = Store::open_in_memory().expect("store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("save");
    store
}

/// The headline: an agent asking `dead` gets the components too.
#[test]
fn a_dead_query_carries_the_abandoned_cycles_of_its_generation() {
    let store = persisted();
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");

    let clusters = response
        .dead_clusters
        .as_ref()
        .expect("a generation this build wrote must carry a cluster scan");
    assert_eq!(
        clusters.len(),
        1,
        "the ring is one component, not three findings: {clusters:?}"
    );
    let cluster = &clusters[0];
    assert_eq!(cluster.size, 3, "the size is the real membership");
    for member in [
        "app/ring.py::_alpha",
        "app/ring.py::_beta",
        "app/ring.py::_gamma",
    ] {
        assert!(
            cluster.members.iter().any(|name| name == member),
            "{member} must be named: {cluster:?}"
        );
    }
    assert!(
        cluster.confidence > 0.0 && cluster.confidence < 0.9,
        "a whole-graph claim is weaker than a one-symbol one and stronger than \
         nothing: {cluster:?}"
    );
    assert_eq!(
        response.dead_clusters_truncated, 0,
        "one component is under any cap"
    );
}

/// And the two populations really are disjoint — which is why dropping one of
/// them was a recall hole rather than a formatting choice.
///
/// Every member of the ring has an inbound call edge, so `dead_symbols` cannot
/// name any of them at any confidence. If this ever stops being true the
/// cluster pass has become a duplicate and should be argued for again.
#[test]
fn the_single_symbol_list_cannot_name_a_cluster_member() {
    let store = persisted();
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");

    let named: Vec<&str> = response
        .items
        .iter()
        .map(|report| report.symbol_name.as_str())
        .collect();
    for member in ["_alpha", "_beta", "_gamma"] {
        assert!(
            !named.contains(&member),
            "{member} has an inbound edge from its neighbour, so the one-hop \
             join must not report it — and therefore the cluster list is the \
             only place it can appear: {named:?}"
        );
    }
}

/// A generation with no clusters must say so as an empty list, not as an
/// absence.
///
/// "The pass ran and found none" is a finding. "This generation predates the
/// pass" is not, and an agent deciding whether to rebuild needs the difference.
#[test]
fn a_clean_corpus_reports_an_empty_scan_and_not_a_missing_one() {
    let extractions = vec![extract_file(
        "app/main.py",
        "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n",
    )];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    assert!(analysis.dead_clusters.clusters.is_empty());

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");

    match response.dead_clusters {
        Some(clusters) => assert!(
            clusters.is_empty(),
            "a corpus with no cycles reports none: {clusters:?}"
        ),
        None => panic!(
            "an empty scan must arrive as `Some([])`; `None` says the pass did \
             not run, which is a different answer"
        ),
    }
}

/// An empty store answers `Unavailable` and carries no cluster claim either.
///
/// The OFF direction for the field itself: a response that cannot answer must
/// not arrive holding an empty list, which would read as "no abandoned cycles".
#[test]
fn an_unavailable_answer_makes_no_cluster_claim() {
    let store = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");
    assert!(
        response.dead_clusters.is_none(),
        "nothing was read, so nothing is known about components either: {:?}",
        response.dead_clusters
    );
}

/// The response survives the wire it travels on.
///
/// `IpcCommand::Dead` serialises this exact value, and `DevMapClient` parses it.
/// A field that round-trips wrongly is worse than one that is absent, because
/// the absence is visible.
#[test]
fn the_cluster_list_round_trips_through_the_wire_form() {
    let store = persisted();
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");

    let wire = serde_json::to_value(&response).expect("serialise");
    let clusters = wire
        .get("dead_clusters")
        .and_then(|value| value.as_array())
        .expect("`dead_clusters` must be on the wire form");
    assert_eq!(clusters.len(), 1);
    let cluster = &clusters[0];
    for key in ["cluster_id", "members", "size", "confidence", "reason"] {
        assert!(
            cluster.get(key).is_some(),
            "a wire cluster must carry {key}: {cluster:?}"
        );
    }

    // And a response with nothing to say omits the key entirely rather than
    // sending `null`, so a consumer that predates the field is unaffected.
    let store = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&store);
    let empty = engine.dead_symbols(2_000).expect("dead");
    let wire = serde_json::to_value(&empty).expect("serialise");
    assert!(
        wire.get("dead_clusters").is_none(),
        "an absent scan must not become a JSON null: {wire:?}"
    );
    assert!(
        wire.get("dead_clusters_truncated").is_none(),
        "and a zero truncation count is not news: {wire:?}"
    );
}

/// The third outcome, and the one an `Option<Vec<_>>` cannot hold.
///
/// `DeadClusterScan` has a `refused_oversized_graph` flag: past
/// `DEAD_CLUSTER_MAX_NODES` the walk does not run, and `clusters` is empty
/// *because nothing looked*. Mapping the struct field-for-field therefore turns
/// "too large to walk" into `Some([])` — "the pass ran and found no abandoned
/// subsystems" — which is the strongest available reading of the weakest
/// available evidence, and the one an agent acts on by deleting nothing and
/// believing the subsystem is live.
///
/// The refusal is reached by flag rather than by a 400,000-symbol corpus on
/// purpose: `graded_cap_under_hostile_input.rs` already pins *when* the kernel
/// refuses. What is unpinned, and what this asserts, is what the query surface
/// does with a refusal once it has one.
#[test]
fn a_refused_scan_arrives_as_an_absence_with_a_reason_not_as_an_empty_finding() {
    let extractions = extractions();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let mut analysis = analyze(&extractions, &resolution);
    assert!(
        !analysis.dead_clusters.clusters.is_empty(),
        "the fixture found clusters before the refusal was stamped, so the \
         emptiness below is the refusal's doing and not the corpus's"
    );
    // Exactly the shape the kernel writes when it bails: the flag set and the
    // list empty. Leaving the found clusters in place would test a state that
    // cannot occur and would let a field-for-field mapping pass.
    analysis.dead_clusters.clusters.clear();
    analysis.dead_clusters.truncated_clusters = 0;
    analysis.dead_clusters.refused_oversized_graph = true;

    let store = Store::open_in_memory().expect("store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("save");
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");

    assert!(
        response.dead_clusters.is_none(),
        "a walk that did not run knows nothing about components; an empty list \
         would say it ran: {:?}",
        response.dead_clusters
    );
    let reason = response.dead_clusters_incomplete.as_deref().expect(
        "a refusal must say why, or it is indistinguishable from a \
                 generation that predates the pass — and those take opposite \
                 advice",
    );
    assert!(
        reason.contains(&devmap_analyze::dead_clusters::DEAD_CLUSTER_MAX_NODES.to_string()),
        "the reason must name the ceiling that was hit, so the reader can tell \
         whether their graph is near it: {reason:?}"
    );
    assert_eq!(
        response.dead_clusters_truncated, 0,
        "nothing was cut by a cap; nothing was walked at all"
    );

    // And on the wire, where every downstream consumer reads it.
    let wire = serde_json::to_value(&response).expect("serialise");
    assert!(
        wire.get("dead_clusters").is_none(),
        "an absent list must not become a JSON null or an empty array: {wire:?}"
    );
    assert!(
        wire.get("dead_clusters_incomplete")
            .and_then(|value| value.as_str())
            .is_some_and(|text| text == reason),
        "the reason travels with the absence: {wire:?}"
    );
}

/// The OFF direction for the same field: a scan that *did* run carries no
/// reason.
///
/// Without this, setting the string unconditionally — the cheapest way to make
/// the test above pass — would leave every ordinary answer carrying a caveat,
/// and a caveat on every answer is one nobody reads on the answer that needs it.
#[test]
fn a_scan_that_ran_carries_no_incompleteness_reason() {
    let store = persisted();
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");
    assert!(
        response.dead_clusters.is_some(),
        "precondition: this generation has a scan"
    );
    assert_eq!(
        response.dead_clusters_incomplete, None,
        "a completed walk has nothing to disclose: {:?}",
        response.dead_clusters_incomplete
    );

    // And the empty store, which is a third kind of absence again: nothing was
    // read, so there is no refusal to report either.
    let empty = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&empty);
    let response = engine.dead_symbols(2_000).expect("dead");
    assert_eq!(
        response.dead_clusters_incomplete, None,
        "no generation was read; naming a cause would name one this path does \
         not know: {:?}",
        response.dead_clusters_incomplete
    );
}

/// A capped list must never arrive as a complete one.
///
/// `DEAD_CLUSTER_CAP` is 50 and the producer sorts largest-first before cutting,
/// so a repository with more abandoned components than that gets a *sample* —
/// and the count of what was cut is the only thing separating "these are the
/// abandoned subsystems" from "these are fifty of them". The truncation counter
/// travels beside the list rather than inside `hidden`, which counts what the
/// token budget trimmed, so nothing else on the response can carry it.
#[test]
fn a_capped_cluster_list_carries_what_the_cap_cut() {
    // Sixty independent two-function cycles: each file's pair calls only each
    // other and nothing outside reaches either, so each is its own component.
    let sources: Vec<(String, String)> = (0..60)
        .map(|index| {
            (
                format!("app/ring_{index}.py"),
                format!(
                    "def _first_{index}():\n    return _second_{index}()\n\n\n\
                     def _second_{index}():\n    return _first_{index}()\n"
                ),
            )
        })
        .collect();
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    assert_eq!(
        analysis.dead_clusters.clusters.len(),
        devmap_analyze::DEAD_CLUSTER_CAP,
        "fixture assumption: the corpus must exceed the producer's cap, or the \
         truncation this asserts never happens: {:?}",
        analysis.dead_clusters.truncated_clusters
    );

    let store = Store::open_in_memory().expect("store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("save");
    let engine = StoreQueryEngine::new(&store);
    let response = engine.dead_symbols(2_000).expect("dead");

    let clusters = response.dead_clusters.as_ref().expect("a scan ran");
    assert_eq!(clusters.len(), devmap_analyze::DEAD_CLUSTER_CAP);
    assert_eq!(
        response.dead_clusters_truncated,
        60 - devmap_analyze::DEAD_CLUSTER_CAP,
        "the ten components the cap dropped must be counted, not forgotten: \
         {response:?}"
    );
    // And the count is not silently folded into the budget's own counters,
    // which are enforced against `shown + hidden == total` by every client.
    assert_eq!(
        response.shown + response.hidden,
        response.total,
        "the cluster cap must not disturb the budget invariant"
    );

    let wire = serde_json::to_value(&response).expect("serialise");
    assert_eq!(
        wire.get("dead_clusters_truncated").and_then(|v| v.as_u64()),
        Some(10),
        "a consumer reading only the wire form must still see the cut: {wire:?}"
    );
}
