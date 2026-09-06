//! What a generation could not read, kept as an inventory rather than a count.
//!
//! Three kinds of hole, one table. A file discovery refused never reached an
//! extractor at all; a file a grammar was wanted for and did not read left a
//! `File` node and nothing else; a file recovered by line pattern left names
//! and spans but no calls. All three cap the dead-code pass below the confident
//! tier, and until now all three were reported as bare numbers — "2 file(s)
//! failed to parse, 1 recovered by pattern, 1 refused by discovery" — with
//! nothing anywhere naming a path. On this repository that one refusal is a
//! 30.6 MB vendored `parser.c` against a 1 MiB ceiling, which is the correct
//! verdict; the gap was that nothing said so, and an operator could not tell a
//! correct refusal from a broken one without opening the database by hand.
//!
//! The refusal half also has to be *maintained*, not merely reported: the
//! daemon's incremental drain never re-walks discovery, so it carries the
//! previous generation's verdicts forward. A count cannot be maintained — see
//! `schema::COVERAGE_GAPS_TABLE` for what carrying one instead cost.

use devmap_analyze::ExtractionGap;
use devmap_extract::model::DiscoveryReport;

/// The stored `gap` value for a path discovery refused before extraction.
///
/// Not an [`ExtractionGap`] variant: those two are decided by looking at an
/// `Extraction`, and a refused path has none. Kept in the same table because
/// the question a consumer asks is one question — what did this generation not
/// read — and because `status` reports the three side by side.
pub const GAP_DISCOVERY_REFUSED: &str = "discovery_refused";

/// Most paths of one kind that a status answer carries.
///
/// A sample, and every surface that renders it carries the true total beside
/// it. `Store::DEGRADED_SAMPLE` is 5 for quarantined paths, which is a queue an
/// operator drains; this is a corpus inventory that is read once and acted on,
/// so it is wider.
pub const COVERAGE_GAP_SAMPLE: usize = 50;

/// One path a generation could not read, and the verdict that says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRefusal {
    pub path: String,
    /// `DiscoverySkipReason`'s own `Display`, which is the one wording the cold
    /// walk and the drain share.
    pub reason: String,
}

/// One row of a coverage-gap listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageGapRow {
    pub path: String,
    pub reason: String,
}

/// A capped listing and the total it was drawn from.
///
/// Never just the rows: a list that stops at fifty and does not say so reads
/// exactly like a corpus with fifty holes in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageGapSample {
    /// Every row of this kind in the generation.
    pub total: usize,
    /// At most [`COVERAGE_GAP_SAMPLE`] of them, by path.
    pub shown: Vec<CoverageGapRow>,
}

impl CoverageGapSample {
    pub fn truncated(&self) -> bool {
        self.shown.len() < self.total
    }
}

/// What a generation could not read, in the three kinds it can fail to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageGaps {
    pub discovery_refused: CoverageGapSample,
    pub parse_failed: CoverageGapSample,
    pub pattern_recovered: CoverageGapSample,
}

impl CoverageGaps {
    /// The sample for one stored `gap` label, or `None` for a label this
    /// binary does not know.
    ///
    /// Named rather than matched inline so a label added to the table stops
    /// compiling in one place instead of silently landing nowhere.
    pub(crate) fn slot(&mut self, gap: &str) -> Option<&mut CoverageGapSample> {
        if gap == GAP_DISCOVERY_REFUSED {
            Some(&mut self.discovery_refused)
        } else if gap == ExtractionGap::ParseFailed.label() {
            Some(&mut self.parse_failed)
        } else if gap == ExtractionGap::PatternRecovered.label() {
            Some(&mut self.pattern_recovered)
        } else {
            None
        }
    }

    /// The labels a reader asks for, in the order `status` reports them.
    pub(crate) fn labels() -> [&'static str; 3] {
        [
            GAP_DISCOVERY_REFUSED,
            ExtractionGap::ParseFailed.label(),
            ExtractionGap::PatternRecovered.label(),
        ]
    }
}

/// The refusal inventory a cold walk's report establishes.
///
/// `DiscoveryReport::refusals` decides *which* skips are coverage loss — the
/// one owner of that line — and this only puts them in the shape the store
/// persists, so the walk and the inventory cannot disagree about what a
/// refusal is. The reason is rendered through `Display` rather than `Debug`
/// because that is the wording the drain already logs and `devmap build`
/// already prints.
pub fn discovery_refusals(report: &DiscoveryReport) -> Vec<DiscoveryRefusal> {
    report
        .refusals()
        .map(|(path, reason)| DiscoveryRefusal {
            path: path.clone(),
            reason: reason.to_string(),
        })
        .collect()
}
