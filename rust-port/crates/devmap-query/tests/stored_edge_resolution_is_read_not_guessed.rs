//! Schema 15 persists each edge's `Resolution` *kind* in
//! `generation_edges.resolution`. `devmap_query::resolved_edge_from_stored` is
//! the path `devmap manifest` builds `code_graph.json` from, and until this
//! test it set `resolution: None` on every edge and carried nothing else — so
//! the artifact every agent reads asserted confidences whose justification
//! nothing could read back.
//!
//! What a row can answer is the kind and where it came from, not the payload
//! (an `ImportScoped` row does not carry `imported_from`), so a re-read edge
//! fills `ResolvedEdge::evidence` and leaves `resolution` `None` rather than
//! inventing a variant to fill it. A reconstruction — a row from before the
//! column — comes back labelled `Reconstructed`; that half is pinned in
//! `devmap-store/tests/coverage_gap_inventory.rs`, which can clear the column.
//!
//! Red against the pre-fix tree: `evidence` was `None` on every re-read edge.

use std::sync::Arc;

use devmap_extract::model::EdgeKind;
use devmap_resolve::model::{
    Evidence, Resolution, ResolutionKind, ResolutionResult, ResolutionSource, ResolvedEdge,
};
use devmap_store::Store;

#[test]
fn a_re_read_edge_carries_the_evidence_the_resolver_recorded() {
    let store = Store::open_in_memory().unwrap();
    let extractions = vec![devmap_extract::extract_file(
        "geo/measure.py",
        "def area():\n    return 1\n\n\ndef perimeter():\n    return area()\n",
    )];
    let resolution = ResolutionResult {
        edges: vec![ResolvedEdge::resolved(
            "geo/measure.py".to_string(),
            "geo/measure.py".to_string(),
            "perimeter".to_string(),
            "area".to_string(),
            EdgeKind::MemberOf,
            Arc::new(Resolution::Structural {
                target_symbol: "area".to_string(),
                target_file: "geo/measure.py".to_string(),
            }),
            None,
        )],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let edges: Vec<ResolvedEdge> = store
        .latest_edges(0.0)
        .unwrap()
        .into_iter()
        .map(devmap_query::resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(edges.len(), 1);

    let edge = &edges[0];
    assert_eq!(
        edge.evidence,
        Some(Evidence {
            kind: ResolutionKind::Structural,
            source: ResolutionSource::Stored,
        }),
        "the row's own file layout says `SameFile` — the two endpoints share a \
         file — and the resolver said `Structural`. An edge rebuilt from the row \
         must carry what the resolver recorded, labelled as read: {:?}",
        edge.evidence
    );
    assert!(
        edge.resolution.is_none(),
        "the payload is not persisted, so rebuilding the variant would be a guess \
         wearing the resolver's type: {:?}",
        edge.resolution
    );
    assert_eq!(
        edge.confidence,
        ResolutionKind::Structural.confidence(),
        "the confidence read back is the one the stored kind entitles"
    );
}
