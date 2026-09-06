//! Adversarial input for the W2.3 rung filter.
//!
//! The histogram's job is to let a caller tell a filtered answer from a sparse
//! graph. Every property below is one where getting it wrong reintroduces
//! exactly that confusion — a count that drifts from the population it
//! describes is worse than no count, because it looks authoritative.

use devmap_extract::model::{Confidence, EdgeKind};
use devmap_query::rung::{filter_by_rung, histogram, narrow, Rung, RungHistogram};
use devmap_resolve::model::ResolvedEdge;

fn edge(confidence: f32, index: usize) -> ResolvedEdge {
    ResolvedEdge {
        source_file: format!("src/f{}.py", index % 97),
        source_symbol: format!("caller_{index}"),
        target_file: format!("src/t{}.py", index % 89),
        target_symbol: format!("callee_{index}"),
        edge_kind: EdgeKind::Calls,
        confidence: Confidence(confidence),
        resolution: None,
        evidence: None,
        details: None,
    }
}

/// The invariant the whole field rests on: nothing is lost and nothing is
/// invented, at any population size.
#[test]
fn the_histogram_accounts_for_every_edge_at_scale() {
    let confidences = [1.0_f32, 0.9, 0.7, 0.4, 0.2, 0.0, 0.95, 0.699_999, 0.400_001];
    for size in [0usize, 1, 2, 999, 100_000] {
        let edges: Vec<ResolvedEdge> = (0..size)
            .map(|i| edge(confidences[i % confidences.len()], i))
            .collect();
        for floor in [
            None,
            Some(Rung::Deterministic),
            Some(Rung::High),
            Some(Rung::Speculative),
        ] {
            let (kept, counts) = narrow(edges.clone(), floor);
            assert_eq!(
                counts.total(),
                size,
                "size={size} floor={floor:?}: the histogram must count the whole \
                 population"
            );
            assert_eq!(
                counts.total() - counts.filtered_out,
                kept.len(),
                "size={size} floor={floor:?}: every edge is either kept or counted \
                 as filtered out"
            );
            if let Some(rung) = floor {
                for surviving in &kept {
                    assert!(
                        rung.admits(devmap_extract::model::confidence_millis(
                            surviving.confidence.0
                        )),
                        "an edge below the floor survived it"
                    );
                }
            } else {
                assert_eq!(kept.len(), size, "no floor must filter nothing");
            }
        }
    }
}

/// Confidences the ladder never names must still land on exactly one rung.
///
/// A persisted value is a float that made a round trip through SQLite REAL, so
/// it is not guaranteed to equal any constant. Every one of them has to bucket,
/// and none may bucket twice.
#[test]
fn every_representable_confidence_lands_on_exactly_one_rung() {
    let mut probes: Vec<f32> = vec![
        f32::MIN_POSITIVE,
        -0.0,
        0.0,
        0.399_999_97,
        0.4,
        0.400_000_03,
        0.699_999_94,
        0.7,
        0.700_000_1,
        0.899_999_94,
        0.9,
        0.900_000_04,
        0.999_999_94,
        1.0,
    ];
    // Values outside [0, 1] should not exist, but a corrupted store can hold
    // them and a panic there would take down every query over that generation.
    probes.extend([-1.0, 2.0, f32::MAX, f32::MIN]);

    for probe in probes {
        let millis = devmap_extract::model::confidence_millis(probe);
        let rung = Rung::of_millis(millis);
        let admitting: Vec<Rung> = Rung::ALL
            .iter()
            .copied()
            .filter(|candidate| candidate.admits(millis))
            .collect();
        assert!(
            admitting.contains(&rung),
            "{probe} ({millis}) bucketed as {rung:?} but that rung does not admit it"
        );
        // Weaker floors always admit what a stronger one does — the ladder is
        // ordered, and a filter that was not would be incoherent.
        assert!(
            admitting.windows(2).all(|pair| pair[0] <= pair[1]),
            "{probe}: admission is not monotonic across the ladder: {admitting:?}"
        );
    }
}

/// NaN must bucket, not panic and not vanish.
///
/// It cannot be ordered against anything, so the only safe answer is the
/// weakest rung: a filter that dropped it silently would shrink the population
/// without telling anybody, which is the failure the histogram exists to
/// prevent.
#[test]
fn a_nan_confidence_is_counted_rather_than_dropped() {
    let edges = vec![edge(f32::NAN, 0), edge(1.0, 1)];
    for floor in [None, Some(Rung::Deterministic), Some(Rung::Speculative)] {
        let (kept, counts) = narrow(edges.clone(), floor);
        assert_eq!(counts.total(), 2, "floor={floor:?}: NaN must still be counted");
        assert_eq!(counts.total() - counts.filtered_out, kept.len());
    }
}

/// `filter_by_rung` keeps input order, which several callers rely on.
///
/// The engine sorts by confidence *before* filtering, so a filter that
/// reordered would hand back a differently-ranked answer than the unfiltered
/// query for the same graph.
#[test]
fn filtering_preserves_order() {
    let edges: Vec<ResolvedEdge> = (0..500)
        .map(|i| edge(if i % 3 == 0 { 1.0 } else { 0.2 }, i))
        .collect();
    let kept = filter_by_rung(edges.clone(), Rung::Deterministic);
    let expected: Vec<&str> = edges
        .iter()
        .filter(|e| e.confidence.0 >= 1.0)
        .map(|e| e.source_symbol.as_str())
        .collect();
    let got: Vec<&str> = kept.iter().map(|e| e.source_symbol.as_str()).collect();
    assert_eq!(got, expected);
}

/// An empty population reports zeroes, and zeroes are not a filter's work.
#[test]
fn an_empty_population_reports_zeroes_and_no_filtering() {
    let counts = histogram(&[], Some(Rung::Deterministic));
    assert_eq!(counts, RungHistogram::default());
    assert_eq!(counts.filtered_out, 0, "nothing was filtered because nothing was there");
    assert_eq!(counts.total(), 0);
}
