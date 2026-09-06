//! `ResolutionKind` is the one owner of three things that used to live in two
//! crates: the variant set of `Resolution`, the confidence each rung entitles,
//! and the spelling the store persists. These pin the three against each other
//! so a drift in any one is a red test rather than a silent tier change.

use std::sync::Arc;

use devmap_extract::model::{Confidence, EdgeKind};
use devmap_resolve::model::{
    Evidence, LangFamily, Resolution, ResolutionKind, ResolutionSource, ResolvedEdge,
};

fn every_variant() -> Vec<Resolution> {
    let file = "geo/measure.py".to_string();
    let symbol = "area".to_string();
    vec![
        Resolution::SameFile {
            target_symbol: symbol.clone(),
            target_file: file.clone(),
        },
        Resolution::ImportScoped {
            target_symbol: symbol.clone(),
            target_file: file.clone(),
            imported_from: "geo".to_string(),
        },
        Resolution::ReceiverType {
            target_symbol: symbol.clone(),
            target_file: file.clone(),
            receiver_type: "Shape".to_string(),
        },
        Resolution::UniqueGlobal {
            target_symbol: symbol.clone(),
            target_file: file.clone(),
            family: LangFamily::Python,
        },
        Resolution::AmbiguousGlobal {
            candidates: vec![(file.clone(), symbol.clone())],
            family: LangFamily::Python,
        },
        Resolution::Unresolved {
            reason: "no declaration".to_string(),
        },
        Resolution::Structural {
            target_symbol: symbol,
            target_file: file,
        },
    ]
}

#[test]
fn every_resolution_variant_has_a_kind_and_the_kinds_are_exactly_the_variants() {
    let kinds: Vec<ResolutionKind> = every_variant().iter().map(Resolution::kind).collect();
    assert_eq!(
        kinds,
        ResolutionKind::ALL.to_vec(),
        "`ALL` must list every kind once, in the order `kind()` produces them"
    );
}

#[test]
fn a_resolution_scores_exactly_what_its_kind_scores() {
    for resolution in every_variant() {
        assert_eq!(
            resolution.confidence(),
            resolution.kind().confidence(),
            "{resolution:?}: the payload must not change the tier"
        );
    }
    // The ladder itself, pinned: a change here is a change to every stored
    // confidence in every store, and must be made on purpose.
    assert_eq!(
        ResolutionKind::SameFile.confidence(),
        Confidence::DETERMINISTIC
    );
    assert_eq!(
        ResolutionKind::ImportScoped.confidence(),
        Confidence::DETERMINISTIC
    );
    assert_eq!(
        ResolutionKind::ReceiverType.confidence(),
        Confidence::DETERMINISTIC
    );
    assert_eq!(
        ResolutionKind::Structural.confidence(),
        Confidence::DETERMINISTIC
    );
    assert_eq!(ResolutionKind::UniqueGlobal.confidence(), Confidence::HIGH);
    assert_eq!(
        ResolutionKind::AmbiguousGlobal.confidence(),
        Confidence::SPECULATIVE
    );
    assert_eq!(
        ResolutionKind::Unresolved.confidence(),
        Confidence::SPECULATIVE
    );
}

#[test]
fn the_stored_spelling_round_trips_and_an_unknown_one_is_refused() {
    for kind in ResolutionKind::ALL {
        assert_eq!(
            ResolutionKind::from_label(kind.label()),
            Some(kind),
            "{kind:?} must read back as itself"
        );
    }
    let labels: std::collections::BTreeSet<&str> = ResolutionKind::ALL
        .iter()
        .map(|kind| kind.label())
        .collect();
    assert_eq!(
        labels.len(),
        ResolutionKind::ALL.len(),
        "two kinds share a spelling"
    );
    assert_eq!(
        ResolutionKind::from_label("samefile"),
        None,
        "spellings are exact"
    );
    assert_eq!(ResolutionKind::from_label("Structurally"), None);
    assert_eq!(ResolutionKind::from_label(""), None);
}

#[test]
fn an_edge_the_resolver_builds_carries_its_evidence_as_resolved() {
    let resolution = Arc::new(Resolution::Structural {
        target_symbol: "area".to_string(),
        target_file: "geo/measure.py".to_string(),
    });
    let edge = ResolvedEdge::resolved(
        "geo/measure.py".to_string(),
        "geo/measure.py".to_string(),
        "perimeter".to_string(),
        "area".to_string(),
        EdgeKind::MemberOf,
        Arc::clone(&resolution),
        None,
    );
    assert_eq!(
        edge.evidence,
        Some(Evidence {
            kind: ResolutionKind::Structural,
            source: ResolutionSource::Resolver,
        })
    );
    assert_eq!(edge.confidence, resolution.confidence());
    assert_eq!(edge.resolution.as_deref(), Some(&*resolution));
    let labels: Vec<&str> = [
        ResolutionSource::Resolver,
        ResolutionSource::Stored,
        ResolutionSource::Reconstructed,
    ]
    .iter()
    .map(|source| source.label())
    .collect();
    assert_eq!(labels, ["resolver", "stored", "reconstructed"]);
}
