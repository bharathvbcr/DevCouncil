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
pub use liveness::{analyze_liveness, GO_BUILD_VARIANT_REASON};
pub use model::*;
pub use pdg::*;
pub use traversal::*;

use devmap_extract::model::*;
use devmap_resolve::model::*;

pub fn analyze(extractions: &[Extraction], resolution: &ResolutionResult) -> AnalysisSummary {
    let dead_symbols = analyze_liveness(extractions, resolution);
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
        status: match detection.degraded {
            None => AnalysisStatus::Ok,
            Some(reason) => AnalysisStatus::Partial { reason },
        },
        unresolved_calls: resolution.unresolved.len(),
        clone_coverage,
    }
}
