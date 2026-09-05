//! K-A2, discovery half: a file the indexer refused to read is a hole in
//! coverage, and every conclusion drawn over that corpus is a lower bound.
//!
//! The parse-failure half of this rule was closed first, and closing it made
//! this half easy to miss: every coverage check was computed from the
//! `&[Extraction]` slice, and a file discovery turned away — oversized,
//! unreadable, a non-UTF-8 path — has no `Extraction` in that slice at all.
//! So the check ran, found every file it could see intact, and reported a
//! complete corpus.
//!
//! The audit measured what that costs. `lib.py` defines `helper()`; its only
//! caller `app.py` is over `MAX_SOURCE_BYTES`, so discovery refuses it:
//!
//! ```text
//! $ devmap build …
//!   discovery refused 1 file(s) — these are absent from the graph
//! $ devmap dead
//!   {"confidence":0.9,"symbol_name":"helper","resolution":"Available","truncated":false}
//! $ devmap --json status
//!   {"degraded_reason":null,"is_fresh":true}
//! ```
//!
//! 0.9 is the confident tier. The map proposed deleting a live function because
//! the file that calls it was never read, and said nothing anywhere about the
//! file it had not read.

use devmap_analyze::model::AnalysisStatus;
use devmap_analyze::{analyze, analyze_with_discovery, DiscoveryCoverage};
use devmap_extract::extract_file;
use devmap_resolve::Resolver;

/// The corpus as the analysis actually sees it: `lib.py` alone. `app.py` is
/// absent precisely because discovery refused it — that absence is the defect.
fn corpus() -> Vec<devmap_extract::model::Extraction> {
    vec![extract_file(
        "lib.py",
        "def helper(rows):\n    return sum(rows)\n",
    )]
}

fn summarize(discovery: DiscoveryCoverage) -> devmap_analyze::model::AnalysisSummary {
    let extractions = corpus();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    analyze_with_discovery(&extractions, &resolution, discovery)
}

#[test]
fn a_refused_file_makes_the_analysis_partial() {
    let summary = summarize(DiscoveryCoverage::refused(1));

    let reason = match &summary.status {
        AnalysisStatus::Partial { reason } => reason.clone(),
        other => panic!(
            "one file was refused by discovery and never read, so this corpus was not fully \
             examined; reporting {other:?} is a check that could not run answering like one \
             that ran and passed"
        ),
    };
    assert!(
        reason.contains("refused by discovery"),
        "the reason must name what was missed, or a reader cannot tell this from a \
         clustering failure: {reason:?}"
    );
}

/// The marker must be absent when nothing was refused, or it means nothing.
#[test]
fn a_fully_discovered_corpus_is_not_marked_partial() {
    let summary = summarize(DiscoveryCoverage::none());
    assert!(
        matches!(summary.status, AnalysisStatus::Ok),
        "nothing was refused and every file parsed, so this corpus was fully examined: {:?}",
        summary.status
    );
}

/// The consequence that matters: a refusal must cap dead-code confidence.
///
/// This is the assertion that would have stopped the audit's proof C. Reporting
/// `Partial` while still offering `helper` at the confident tier would satisfy
/// the first test here and still get a live function deleted.
#[test]
fn a_refused_file_caps_dead_symbol_confidence() {
    let confident = summarize(DiscoveryCoverage::none());
    let capped = summarize(DiscoveryCoverage::refused(1));

    let confident_helper = confident
        .dead_symbols
        .iter()
        .find(|report| report.symbol_name == "helper")
        .map(|report| report.confidence)
        .expect("with nothing refused, `helper` has no callers and is confidently dead");
    let capped_helper = capped
        .dead_symbols
        .iter()
        .find(|report| report.symbol_name == "helper")
        .map(|report| report.confidence)
        .expect(
            "the finding is still reported when coverage is incomplete — downgraded, not hidden",
        );

    assert!(
        capped_helper < confident_helper,
        "a refused file may hold the only call to `helper`, so the finding must be \
         downgraded: {capped_helper} is not below {confident_helper}"
    );
    assert!(
        capped_helper <= devmap_analyze::COVERAGE_LOSS_CONFIDENCE_CAP,
        "the cap the rest of this crate already applies to coverage loss must apply here \
         too: {capped_helper} > {}",
        devmap_analyze::COVERAGE_LOSS_CONFIDENCE_CAP
    );
}

/// `analyze` — the entry point for callers that supply their own corpus — must
/// keep meaning "no discovery step ran", not "discovery refused nothing".
#[test]
fn the_plain_entry_point_still_reports_a_complete_corpus() {
    let extractions = corpus();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let summary = analyze(&extractions, &resolution);
    assert!(
        matches!(summary.status, AnalysisStatus::Ok),
        "a corpus handed over directly had no discovery step to refuse anything: {:?}",
        summary.status
    );
}
