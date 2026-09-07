//! `impact` can say *how far*, so no consumer has to invent it.
//!
//! The flat edge list an `impact` answer carries does not record the hop at
//! which the walk reached each endpoint, so a consumer that needs distance
//! bands cannot derive them — it can only guess. The guess in this repository
//! was `cli/commands/graph_cmd.py`'s kernel branch, which packed every symbol a
//! **depth-3** reverse walk returned into a single band labelled `depth: 1,
//! confidence: "extracted"`: a three-hop transitive dependent published as a
//! direct, deterministically-resolved caller, with nothing in the payload
//! saying otherwise.
//!
//! These tests pin the kernel side of the repair. The bands are the same
//! [`devmap_query::BlastRadius`] `explore` and `affected` already return, they
//! are budgeted out of the caller's own allowance rather than beside it, and a
//! walk that stopped early says so per answer instead of being packed as a
//! complete one.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// `alpha -> beta -> gamma -> delta`: four hops, so an inbound walk from
/// `delta` reaches one distinct symbol at each of depths 1, 2 and 3.
fn chain() -> Vec<Extraction> {
    let files = [
        ("delta.py", "def delta():\n    return 1\n"),
        (
            "gamma.py",
            "from delta import delta\n\n\ndef gamma():\n    return delta()\n",
        ),
        (
            "beta.py",
            "from gamma import gamma\n\n\ndef beta():\n    return gamma()\n",
        ),
        (
            "alpha.py",
            "from beta import beta\n\n\ndef alpha():\n    return beta()\n",
        ),
    ];
    files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

fn store_of(extractions: &[Extraction]) -> Store {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = devmap_analyze::analyze(extractions, &resolution);
    let store = Store::open_in_memory().expect("an in-memory store opens");
    store
        .save_generation_with_opts(
            extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .expect("the generation is written");
    store
}

fn request(target: &str, depth: usize, budget: u32) -> Request<String> {
    Request {
        query: target.to_string(),
        token_budget: budget,
        min_confidence: 0.0,
        max_depth: depth,
    }
}

/// The band a symbol lands in must be the hop the walk actually found it at.
///
/// This is the defect itself, stated positively. `alpha` is three hops from
/// `delta`; the answer that called it a direct caller was not a rounding error
/// but a claim nothing measured.
#[test]
fn each_reached_symbol_is_banded_at_the_hop_it_was_reached() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact_layered(request("delta", 3, 10_000))
        .expect("the store holds a generation");

    let bands = &answer.blast_radius.layers.items;
    assert!(
        bands.len() >= 3,
        "an inbound walk over a four-link chain reaches three distinct depths; \
         got {bands:?}"
    );
    let at = |depth: usize| -> Vec<String> {
        bands
            .iter()
            .find(|layer| layer.depth == depth)
            .map(|layer| layer.nodes.clone())
            .unwrap_or_default()
    };
    let names = |depth: usize| -> Vec<String> {
        at(depth)
            .into_iter()
            .map(|node| node.rsplit("::").next().unwrap_or_default().to_string())
            .collect()
    };

    assert!(
        names(1).contains(&"gamma".to_string()),
        "gamma calls delta directly and belongs at depth 1; depth 1 held {:?}",
        at(1)
    );
    assert!(
        names(2).contains(&"beta".to_string()),
        "beta reaches delta through gamma and belongs at depth 2; depth 2 held {:?}",
        at(2)
    );
    assert!(
        names(3).contains(&"alpha".to_string()),
        "alpha reaches delta through two hops and belongs at depth 3; depth 3 held {:?}",
        at(3)
    );
    // The half that matters. The payload this replaces put all three here.
    assert!(
        !names(1).contains(&"alpha".to_string()),
        "alpha is three hops away and must not be reported as a direct caller; \
         depth 1 held {:?}",
        at(1)
    );
}

/// The bands partition what the walk reached: no double counting, no gaps.
#[test]
fn the_bands_sum_to_the_total_they_are_a_partition_of() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact_layered(request("delta", 3, 10_000))
        .expect("the store holds a generation");
    let layers = &answer.blast_radius.layers;
    assert!(
        !layers.truncated,
        "this budget is large enough to hold every band; a trimmed list would \
         make the sum below a comparison against the wrong number"
    );

    let summed: u32 = layers.items.iter().map(|layer| layer.node_count).sum();
    assert_eq!(
        summed, answer.blast_radius.total_impacted,
        "the per-depth counts must account for every symbol the walk reached, \
         once each: bands {:?}",
        layers.items
    );

    let mut seen = std::collections::BTreeSet::new();
    for layer in &layers.items {
        for node in &layer.nodes {
            assert!(
                seen.insert(node.clone()),
                "{node} appears in more than one band; a symbol is reached at one \
                 minimum distance, and counting it twice inflates the radius"
            );
        }
    }
}

/// A walk stopped by the depth bound must say so on the bands, not only on the
/// edge list — the two halves are packed by different budgeters and a consumer
/// that reads only the bands must still be able to tell a capped radius from a
/// small one.
#[test]
fn a_depth_capped_layering_says_so_on_both_halves() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let capped = engine
        .impact_layered(request("delta", 1, 10_000))
        .expect("the store holds a generation");

    let reason = capped
        .blast_radius
        .layers
        .walk_incomplete
        .as_deref()
        .unwrap_or_default();
    assert!(
        reason.contains("depth"),
        "a radius that stopped at depth 1 over a four-link chain must name the \
         cap; got {reason:?} for bands {:?}",
        capped.blast_radius.layers.items
    );
    assert!(
        capped
            .edges
            .walk_incomplete
            .as_deref()
            .unwrap_or_default()
            .contains("depth"),
        "the edge half carries its own reason and must keep carrying it: {:?}",
        capped.edges.walk_incomplete
    );

    // The two halves must carry the *same* verdict, because they are one walk.
    // A consumer reading only the bands and a consumer reading only the edges
    // must not disagree about whether the answer is a lower bound.
    //
    // Note what is deliberately not asserted here: that a walk which ran out of
    // graph reports `None`. On a store-backed walk it does not, and the cause is
    // outside this crate. `traverse_graph_indexed`'s `has_admitted` probe
    // (devmap-analyze/src/traversal.rs) counts a node's neighbours *before* the
    // reverse-direction exclusions a few lines below it, so a leaf symbol whose
    // only remaining inbound edge is the `Contains` edge from its own file reads
    // as "still expanding" and sets `depth_capped`. The in-memory `QueryEngine`
    // does not show it — the resolution edge list it walks holds no `Contains`
    // — which is why `incomplete_answers_say_so.rs` can assert the `None` this
    // test cannot. It overstates uncertainty rather than understating it, so it
    // is fail-safe; it is still a marker that means less than it says.
    let complete = engine
        .impact_layered(request("delta", 3, 10_000))
        .expect("the store holds a generation");
    let bands_say = complete.blast_radius.layers.walk_incomplete.is_some();
    let edges_say = complete
        .edges
        .walk_incomplete
        .as_deref()
        .unwrap_or_default()
        .contains("did not complete");
    assert_eq!(
        bands_say, edges_say,
        "one walk, one verdict: the bands say incomplete={bands_say} and the edge \
         half says incomplete={edges_say} ({:?} / {:?})",
        complete.blast_radius.layers.walk_incomplete, complete.edges.walk_incomplete
    );
}

/// The bands stop where the caller's depth bound stops.
#[test]
fn no_band_is_reported_past_the_depth_the_caller_asked_for() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    for depth in 1..=3 {
        let answer = engine
            .impact_layered(request("delta", depth, 10_000))
            .expect("the store holds a generation");
        for layer in &answer.blast_radius.layers.items {
            assert!(
                layer.depth <= depth,
                "a depth-{depth} request returned a band at depth {}",
                layer.depth
            );
        }
    }
}

/// One allowance, split — not one allowance per half.
///
/// A composed answer that spent the caller's budget twice would be refused by
/// `DevMapClient._budgeted`, which enforces `tokens_used <= budget` against the
/// number the caller sent.
#[test]
fn both_halves_are_paid_for_out_of_one_budget() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let budget = 200u32;
    let answer = engine
        .impact_layered(request("delta", 3, budget))
        .expect("the store holds a generation");

    let spent = answer.edges.tokens_used + answer.blast_radius.layers.tokens_used;
    assert!(
        spent <= budget,
        "the two halves together spent {spent} of a {budget}-token allowance"
    );
    assert!(
        answer.edges.tokens_used <= budget / 2 + budget % 2,
        "the edge half must stay inside its share, spent {}",
        answer.edges.tokens_used
    );
}

/// The edge half is the same answer `impact` gives on its own.
///
/// Checked rather than assumed: the layered surface exists to *add* bands, and
/// a composition that quietly changed the edge list would make the two
/// surfaces disagree about the same graph.
#[test]
fn the_edge_half_is_the_impact_answer_it_claims_to_be() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let budget = 10_000u32;
    let layered = engine
        .impact_layered(request("delta", 3, budget))
        .expect("the store holds a generation");
    // `impact_layered` gives the edges `budget - budget / 2`; ask `impact` for
    // exactly that so the comparison is of the answers and not of two budgets.
    let plain = engine
        .impact(request("delta", 3, budget - budget / 2))
        .expect("the store holds a generation");

    assert_eq!(
        layered.edges.shown, plain.shown,
        "the layered edge half returned a different number of edges than impact"
    );
    assert_eq!(
        layered.edges.total, plain.total,
        "the layered edge half measured a different population than impact"
    );
    let layered_ids: Vec<(String, String)> = layered
        .edges
        .items
        .iter()
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect();
    let plain_ids: Vec<(String, String)> = plain
        .items
        .iter()
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect();
    assert_eq!(
        layered_ids, plain_ids,
        "the layered edge half must be impact's own answer, in impact's own order"
    );
}

/// The wire form is the `impact` object plus one key.
///
/// `LayeredImpact` flattens the response so a client that predates the field
/// reads exactly the payload it always read. If the flatten were ever dropped,
/// every existing key would move under a nested object and every consumer would
/// break at once — silently, for the ones that use `.get`.
#[test]
fn the_wire_form_is_the_impact_object_plus_the_bands() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact_layered(request("delta", 3, 10_000))
        .expect("the store holds a generation");
    let value = serde_json::to_value(&answer).expect("the answer serializes");
    let object = value.as_object().expect("a JSON object");

    for key in [
        "items",
        "shown",
        "hidden",
        "total",
        "truncated",
        "tokens_used",
        "resolution",
    ] {
        assert!(
            object.contains_key(key),
            "the flattened impact response must still carry {key:?}; keys were {:?}",
            object.keys().collect::<Vec<_>>()
        );
    }
    let bands = object
        .get("blast_radius")
        .and_then(|value| value.get("layers"))
        .and_then(|value| value.get("items"))
        .and_then(|value| value.as_array())
        .expect("blast_radius.layers.items is an array");
    assert!(
        bands
            .iter()
            .any(|band| band.get("depth") == Some(&2.into())),
        "the serialized bands must carry the depth they were computed at: {bands:?}"
    );
}

/// A target nothing reaches is an unavailable radius, not an empty one.
#[test]
fn an_unmatched_target_is_carried_rather_than_answered_as_zero() {
    let extractions = chain();
    let store = store_of(&extractions);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact_layered(request("no_such_symbol", 3, 10_000))
        .expect("the store holds a generation");

    assert_eq!(
        answer.blast_radius.unmatched_targets,
        vec!["no_such_symbol".to_string()],
        "a target that matched no traversal start must be named, or a radius \
         computed over nothing reads as one computed over everything"
    );
    assert!(
        matches!(
            answer.blast_radius.layers.resolution,
            devmap_query::ResolutionAvailability::Unavailable { .. }
        ),
        "no seed means nothing looked; that is not the same as nothing found"
    );
}
