//! Adversarial input for the graded coverage ceiling and the component pass.
//!
//! Both now do arithmetic they did not do before: a division, a `powi`, and a
//! strongly-connected-component walk whose node set changed. Arithmetic on
//! numbers that arrive from outside is where a confident answer comes from a
//! computation nobody meant, so each input is attacked here rather than
//! reasoned about.
//!
//! The direction that matters throughout is the **flattering** one. A bug that
//! demotes a finding costs recall; a bug that promotes one proposes deleting
//! working code. Every assertion below is written so the flattering failure is
//! the one that trips it.

use devmap_analyze::dead_clusters::DEAD_CLUSTER_MAX_NODES;
use devmap_analyze::{
    analyze, dead_clusters, DeadClusterScan, DiscoveryCoverage, ExtractionCoverage,
    COVERAGE_LOSS_CONFIDENCE_CAP, HIGHEST_DEGRADED_CONFIDENCE,
};
use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// The ceiling
// ---------------------------------------------------------------------------

/// Counts that would overflow a `usize` sum must not wrap into a small blind
/// share.
///
/// `discovery_refused_files` is folded in from outside the analyzer —
/// `DiscoveryCoverage::refused(n)` takes whatever a caller passes — and it
/// feeds the numerator of a division. A wrapping add would turn "everything was
/// refused" into "almost nothing was", and hand a near-`extracted` ceiling to a
/// corpus nothing read. Release builds wrap silently, so this is the one that
/// would never have shown up in a debug test run.
#[test]
fn a_saturating_blind_count_cannot_wrap_into_a_confident_ceiling() {
    let coverage = ExtractionCoverage {
        parse_failed_files: usize::MAX,
        pattern_recovered_files: usize::MAX,
        call_blind_files: usize::MAX,
        discovery_refused_files: usize::MAX,
        files_with_call_extraction: 1,
        ..ExtractionCoverage::default()
    };
    assert!(!coverage.is_complete());
    let got = coverage.cap(0.9);
    assert!(
        (got - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "a corpus this blind must be on the floor, not wrapped around to \
         confident: {got}"
    );
    assert!(got.is_finite(), "and it must be a number: {got}");
}

/// The same, one field at a time, so a partial fix cannot pass.
#[test]
fn each_blind_counter_alone_saturates() {
    for build in [
        |coverage: &mut ExtractionCoverage| coverage.parse_failed_files = usize::MAX,
        |coverage: &mut ExtractionCoverage| coverage.pattern_recovered_files = usize::MAX,
        |coverage: &mut ExtractionCoverage| coverage.call_blind_files = usize::MAX,
        |coverage: &mut ExtractionCoverage| coverage.discovery_refused_files = usize::MAX,
    ] {
        let mut coverage = ExtractionCoverage {
            files_with_call_extraction: 1_000,
            ..ExtractionCoverage::default()
        };
        build(&mut coverage);
        let got = coverage.cap(0.9);
        assert!(
            got <= COVERAGE_LOSS_CONFIDENCE_CAP + 1e-6,
            "one saturated counter is still a blind corpus: {got} ({coverage:?})"
        );
    }
}

/// A non-finite confidence must not come back non-finite.
///
/// A persisted confidence is a float that made a round trip through SQLite
/// REAL, and `confidence_millis` already guards its own arithmetic against
/// exactly this. `f32::min` returns the *other* operand when one is NaN, which
/// happens to fail closed here — asserted rather than relied on, because that
/// is a property of `min` and not of anything this crate controls.
#[test]
fn a_non_finite_confidence_is_bounded_rather_than_propagated() {
    let coverage = ExtractionCoverage {
        call_blind_files: 1,
        files_with_call_extraction: 999,
        ..ExtractionCoverage::default()
    };
    for input in [f32::NAN, f32::INFINITY, f32::MAX, 1e30] {
        let got = coverage.cap(input);
        assert!(got.is_finite(), "cap({input}) must be a number, got {got}");
        assert!(
            got <= HIGHEST_DEGRADED_CONFIDENCE + 1e-6,
            "and it must respect the ceiling: cap({input}) = {got}"
        );
        let cluster = coverage.cap_cluster(input, 3);
        assert!(cluster.is_finite() && cluster <= HIGHEST_DEGRADED_CONFIDENCE + 1e-6);
    }
    // Negative and zero go through untouched: `min` keeps them, and a finding
    // below the floor was already weaker than the ceiling.
    assert!(coverage.cap(-1.0) <= 0.0);
    assert_eq!(coverage.cap(0.0), 0.0);
}

/// A component larger than any exponent can express must not underflow to a
/// confidence of zero, or to `NaN`.
#[test]
fn an_absurd_cluster_size_stays_inside_the_band() {
    let coverage = ExtractionCoverage {
        call_blind_files: 1,
        files_with_call_extraction: 1_000_000,
        ..ExtractionCoverage::default()
    };
    for size in [0usize, 1, 64, 65, u32::MAX as usize, usize::MAX] {
        let got = coverage.cap_cluster(0.5, size);
        assert!(
            got.is_finite() && (COVERAGE_LOSS_CONFIDENCE_CAP..=0.5).contains(&got),
            "size {size} produced {got}"
        );
    }
}

/// The ceiling is a ratio, so the absolute counts must not change it.
///
/// Ten blind files in a thousand and ten thousand in a million are the same
/// claim, and a formula that let corpus size leak into the answer would make a
/// big repository's findings systematically more confident than a small one's
/// for no reason anybody could defend.
#[test]
fn the_ceiling_depends_on_the_ratio_and_not_on_the_corpus_size() {
    let small = ExtractionCoverage {
        call_blind_files: 10,
        files_with_call_extraction: 990,
        ..ExtractionCoverage::default()
    };
    let large = ExtractionCoverage {
        call_blind_files: 10_000,
        files_with_call_extraction: 990_000,
        ..ExtractionCoverage::default()
    };
    assert!(
        (small.cap(0.9) - large.cap(0.9)).abs() < 1e-4,
        "same ratio, same ceiling: {} vs {}",
        small.cap(0.9),
        large.cap(0.9)
    );
}

/// A discovery that refused everything, with no extraction at all, is the
/// shape a build produces when the walk fails outright — and it divides by a
/// denominator that is not zero only because of the refusals.
#[test]
fn a_corpus_that_is_entirely_refusals_is_on_the_floor() {
    let coverage = ExtractionCoverage {
        discovery_refused_files: 5_000,
        files_with_call_extraction: 0,
        ..ExtractionCoverage::default()
    };
    let got = coverage.cap(0.9);
    assert!(
        (got - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "nothing was read: {got}"
    );
}

/// End to end: a real corpus whose every file is refused must not publish a
/// confident finding, and must not panic.
#[test]
fn an_analysis_told_everything_was_refused_still_answers() {
    let extractions = vec![extract_file("app/lib.py", "def helper():\n    return 1\n")];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let summary = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        DiscoveryCoverage::refused(usize::MAX),
    );
    for report in summary.dead_symbols.iter().filter(|r| !r.is_exempt) {
        assert!(
            report.confidence <= COVERAGE_LOSS_CONFIDENCE_CAP + 1e-6,
            "{report:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The component pass
// ---------------------------------------------------------------------------

fn call_edge(source: &str, target: &str) -> ResolvedEdge {
    ResolvedEdge::resolved(
        "f.py".to_string(),
        "f.py".to_string(),
        source.to_string(),
        target.to_string(),
        EdgeKind::Calls,
        Arc::new(Resolution::Structural {
            target_symbol: target.to_string(),
            target_file: "f.py".to_string(),
        }),
        None,
    )
}

fn scan_ring(nodes: usize) -> DeadClusterScan {
    let edges: Vec<ResolvedEdge> = (0..nodes)
        .map(|index| {
            call_edge(
                &format!("f.py::n{index}"),
                &format!("f.py::n{}", (index + 1) % nodes),
            )
        })
        .collect();
    let resolution = ResolutionResult {
        edges,
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    dead_clusters(&[], &resolution)
}

/// The pathology the iterative Tarjan exists for, driven through the public
/// entry point rather than the internal helper.
///
/// A generated state machine or a mutually recursive parser reaches this shape,
/// and the CLI builds with `panic = "abort"` and has no `catch_unwind` on this
/// path — so a recursive formulation here is a process death on input a user
/// can write.
#[test]
fn a_hundred_thousand_node_ring_is_one_finding_and_does_not_overflow() {
    let scan = scan_ring(100_000);
    assert_eq!(scan.clusters.len(), 1, "one component, one finding");
    assert_eq!(scan.clusters[0].size, 100_000, "the size is exact");
    assert!(
        scan.clusters[0].members.len() < 100,
        "and the member list is a capped sample, not the membership: {}",
        scan.clusters[0].members.len()
    );
    assert!(
        scan.clusters[0].reason.contains("members listed"),
        "which it says: {:?}",
        scan.clusters[0].reason
    );
}

/// The refusal is a stated outcome, never an empty result that reads as "no
/// dead clusters".
#[test]
fn an_oversized_graph_refuses_out_loud() {
    let scan = scan_ring(DEAD_CLUSTER_MAX_NODES + 1);
    assert!(
        scan.refused_oversized_graph,
        "the walk must say it refused: {scan:?}"
    );
    assert!(
        scan.clusters.is_empty() && scan.unreachable_files.is_empty(),
        "and must not present a partial answer as a whole one: {scan:?}"
    );
}

/// Duplicate edges, self-loops and empty names must not produce a finding out
/// of nothing.
#[test]
fn degenerate_edges_do_not_manufacture_a_component() {
    let resolution = ResolutionResult {
        edges: vec![
            // A symbol that calls itself, twice.
            call_edge("f.py::solo", "f.py::solo"),
            call_edge("f.py::solo", "f.py::solo"),
            // An edge whose endpoints are the empty string.
            call_edge("", ""),
        ],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let scan = dead_clusters(&[], &resolution);
    // A self-loop *is* a cycle, so `solo` and the empty name are each a real
    // one-node component. What must not happen is a panic, a duplicate, or a
    // component that names a symbol twice.
    for cluster in &scan.clusters {
        assert_eq!(
            cluster.size,
            cluster.members.len(),
            "a component this small lists every member: {cluster:?}"
        );
        let mut sorted = cluster.members.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), cluster.members.len(), "no duplicates");
    }
}

/// Deterministic across runs, including the new qualification pass.
///
/// The component ids come from first-seen order over `resolution.edges`, which
/// the resolver sorts. A qualification pass that iterated a `HashSet` and let
/// its order reach the output would break the determinism gate in a way no
/// single run could show.
#[test]
fn the_component_scan_is_deterministic() {
    let extractions: Vec<Extraction> = (0..40)
        .map(|index| {
            extract_file(
                &format!("m{index}.py"),
                "def one():\n    return two()\n\n\ndef two():\n    return one()\n",
            )
        })
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let first = analyze(&extractions, &resolution);
    for _ in 0..5 {
        let again = analyze(&extractions, &resolution);
        assert_eq!(
            first.dead_clusters, again.dead_clusters,
            "the component scan must be byte-identical run to run"
        );
        let render = |reports: &[devmap_analyze::DeadSymbolReport]| {
            reports
                .iter()
                .map(|report| {
                    format!(
                        "{}|{}|{}|{}|{:?}",
                        report.file_path,
                        report.symbol_name,
                        report.confidence,
                        report.is_exempt,
                        report.exemption_reason
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(render(&first.dead_symbols), render(&again.dead_symbols));
    }
}

/// A name made entirely of separators must not panic the qualification join.
///
/// `component_is_qualified` splits member ids on `::` and then on `.`; a name
/// that is nothing but those separators exercises every empty-segment branch at
/// once. Reachable from a corrupted store, and from any grammar that hands back
/// an empty identifier.
#[test]
fn a_symbol_named_only_by_separators_does_not_panic_the_join() {
    for name in ["::", ".", "::.", "a::", "::b", "..", "a.b.c::d.e.f", ""] {
        let resolution = ResolutionResult {
            edges: vec![
                call_edge(name, name),
                // …and an ambiguous edge naming the same thing, which is what
                // drives the qualification branch.
                ResolvedEdge::resolved(
                    "f.py".to_string(),
                    "f.py".to_string(),
                    "f.py::caller".to_string(),
                    name.to_string(),
                    EdgeKind::Calls,
                    Arc::new(Resolution::AmbiguousGlobal {
                        candidates: vec![("f.py".to_string(), name.to_string())],
                        family: devmap_resolve::model::LangFamily::Python,
                    }),
                    None,
                ),
            ],
            receiver_types: Default::default(),
            reexport_chains: Default::default(),
            unresolved: Vec::new(),
        };
        let scan = dead_clusters(&[], &resolution);
        for cluster in &scan.clusters {
            assert!(cluster.confidence.is_finite(), "{name:?}: {cluster:?}");
        }
    }
}
