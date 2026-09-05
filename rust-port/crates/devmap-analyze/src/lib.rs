pub mod clones;
pub mod clustering;
pub mod liveness;
pub mod model;
pub mod pdg;
pub mod traversal;

pub use clones::{
    candidates_from_extractions, clone_coverage, group_clones, CloneCandidate, CloneCoverage,
    CloneGroup, CloneKind, CloneMember, CloneSummary,
};
pub use clustering::{detect_communities, CommunityDetection};
pub use liveness::{
    analyze_liveness, analyze_liveness_with_coverage, extraction_coverage, ExtractionCoverage,
    LivenessOutcome, COVERAGE_LOSS_CONFIDENCE_CAP, COVERAGE_LOSS_REASON, GO_BUILD_VARIANT_REASON,
};
pub use model::*;
pub use pdg::*;
pub use traversal::*;

use devmap_extract::model::*;
use devmap_resolve::model::*;

pub fn analyze(extractions: &[Extraction], resolution: &ResolutionResult) -> AnalysisSummary {
    let liveness = analyze_liveness_with_coverage(extractions, resolution);
    let dead_symbols = liveness.reports;
    let detection = detect_communities(extractions, resolution);
    let clone_coverage = clone_coverage(extractions);

    let total_symbols = extractions.iter().map(|e| e.symbols.len()).sum();

    AnalysisSummary {
        total_files: extractions.len(),
        total_symbols,
        total_edges: resolution.edges.len(),
        dead_symbols,
        communities: detection.communities,
        // Computed, not asserted. This field was a literal `Ok` on every path,
        // which made `AnalysisStatus::Partial` and `Timeout` unconstructible
        // and the two arms rendering them unreachable — so N4's acceptance
        // ("a check that could not run never reports as one that passed") was
        // satisfied by a type that existed and a value that never varied.
        //
        // Two independent reasons now reach it, and both must. Clustering
        // convergence answers "did the partition settle"; extraction coverage
        // answers "was every file's calls looked for" — the second was the
        // half nothing folded in, so a generation missing whole files' call
        // edges reported `ok` and drove `graph_degraded = false`, which is what
        // let a maximum-confidence proposal to delete working code out of a
        // check that could not run. Concatenated rather than ranked because a
        // reader acting on `Partial` needs every reason it holds, not the
        // first one that happened to fire.
        status: match combine_reasons(detection.degraded, liveness.coverage.degraded_reason()) {
            None => AnalysisStatus::Ok,
            Some(reason) => AnalysisStatus::Partial { reason },
        },
        unresolved_calls: resolution.unresolved.len(),
        clone_coverage,
    }
}

/// Join two "why this answer is qualified" reasons, keeping `None` when
/// neither fired.
///
/// `None` is load-bearing: here it is the only value that produces
/// `AnalysisStatus::Ok`, and therefore `graph_degraded: false` and
/// `analysis_status: "ok"`. A clean corpus whose partition converged must
/// still report exactly that, or the flag stops meaning anything.
///
/// Concatenated rather than ranked, and public for the same reason: a reader
/// acting on a degraded answer needs every reason it holds, not the first one
/// that happened to fire, and the query layer qualifies its dead-symbol
/// answers with exactly this pair-of-reasons shape.
pub fn combine_reasons(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (None, None) => None,
        (Some(reason), None) | (None, Some(reason)) => Some(reason),
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
    }
}
