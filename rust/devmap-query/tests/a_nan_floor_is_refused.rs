//! A NaN confidence floor is refused by every indexed surface.
//!
//! Moved out of `engine.rs`'s inline `indexed_start_equivalence_tests` module;
//! it needs only the public API, while that module's other cases compare the
//! private indexed walks against the scans they replaced.

use devmap_query::model::Request;
use devmap_query::StoreQueryEngine;
use devmap_store::Store;

/// A confidence no comparison can evaluate is refused, not answered.
///
/// Over an *empty* store deliberately: the refusal has to come before the
/// generation lookup, or a caller who asked an unanswerable question is
/// told about the store instead. That ordering was free while every
/// traversal went through `latest_edges`, which validated first; the index
/// path has to state it.
#[test]
fn a_nan_floor_is_refused_by_every_indexed_surface() {
    let store = Store::open_in_memory().expect("store");
    let engine = StoreQueryEngine::new(&store);
    for request in [
        Request {
            query: "hub".to_string(),
            token_budget: 2_000,
            min_confidence: f32::NAN,
            max_depth: 3,
        },
        Request {
            query: "hub".to_string(),
            token_budget: 2_000,
            min_confidence: f32::NAN,
            max_depth: 1,
        },
    ] {
        assert!(
            engine.impact(request.clone()).is_err(),
            "NaN was answered rather than refused"
        );
    }
    assert!(engine
        .affected_tests(&["hub".to_string()], 2_000, f32::NAN, 3)
        .is_err());
    assert!(engine.explore("hub", 5, 2_000, f32::NAN, 3).is_err());
}
