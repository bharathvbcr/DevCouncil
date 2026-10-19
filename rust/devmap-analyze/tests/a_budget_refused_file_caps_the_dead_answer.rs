//! A file refused by the parse budget is a hole in the corpus, and the dead
//! answer must say so and must not hand out the confident tier.
//!
//! `BUDGET_REFUSAL_PREFIX` is the reason `treesitter` stamps on a
//! `ParseOutcome::Failed` when a parse overran its wall-clock budget. The file
//! contributes a `File` node and nothing else, so any symbol only that file
//! calls looks uncalled. The refusal is injected through the real extractor
//! (`extract_file_with_budget` with a zero budget) rather than by editing a
//! `ParseOutcome`, so the fixture breaks if the extractor's stamp ever stops
//! matching the prefix this crate keys on.

use devmap_analyze::model::AnalysisStatus;
use devmap_analyze::{
    analyze, analyze_liveness_with_coverage, DeadSymbolReport, DiscoveryCoverage,
    HIGHEST_DEGRADED_CONFIDENCE,
};
use devmap_extract::model::{Extraction, ParseOutcome, BUDGET_REFUSAL_PREFIX};
use devmap_extract::{extract_file, extract_file_with_budget};
use devmap_resolve::Resolver;
use std::time::Duration;

// Go, because it is a language whose confident tier is open
// (`CONFIDENT_TIER_ELIGIBLE`): a Python corpus is capped with or without a
// refusal, which would make the clean-corpus comparison below vacuous.
//
// `hidden` is called only from the file that gets refused, so it is exactly the
// symbol a refusal turns into a false "nothing calls this".
const LIB: &str = "package lib\n\nfunc used() int { return 1 }\n\nfunc hidden() int { return 2 }\n\nfunc orphan() int { return 3 }\n";
const CALLER: &str = "package lib\n\nfunc Main() int { return used() }\n";
const REFUSED: &str = "package lib\n\nfunc slow() int { return hidden() }\n";

fn corpus(refuse_one: bool) -> Vec<Extraction> {
    let mut extractions = vec![extract_file("lib.go", LIB), extract_file("app.go", CALLER)];
    extractions.push(if refuse_one {
        let refused = extract_file_with_budget("slow.go", REFUSED, Duration::ZERO);
        assert!(refused.is_budget_refusal(), "the fixture must be a refusal");
        match &refused.parse_outcome {
            ParseOutcome::Failed { reason } => assert!(
                reason.starts_with(BUDGET_REFUSAL_PREFIX),
                "the refusal carries the shared prefix: {reason}"
            ),
            other => panic!("a budget refusal is a Failed outcome: {other:?}"),
        }
        refused
    } else {
        extract_file("slow.go", REFUSED)
    });
    extractions
}

fn resolved(extractions: &[Extraction]) -> devmap_resolve::model::ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    resolver.resolve_all(extractions).unwrap()
}

fn findings(extractions: &[Extraction]) -> Vec<DeadSymbolReport> {
    let outcome = analyze_liveness_with_coverage(
        extractions,
        &resolved(extractions),
        DiscoveryCoverage::none(),
    );
    outcome
        .reports
        .into_iter()
        .filter(|report| !report.is_exempt)
        .collect()
}

fn status_of(extractions: &[Extraction]) -> AnalysisStatus {
    analyze(extractions, &resolved(extractions)).status
}

#[test]
fn a_budget_refused_file_degrades_the_analysis_and_is_counted() {
    let AnalysisStatus::Partial { reason } = status_of(&corpus(true)) else {
        panic!("a corpus with a refused file must not read as an `ok` analysis");
    };
    assert!(
        reason.contains("1 file(s) failed to parse"),
        "the reason names the file count: {reason}"
    );
    // Asserted against the clause's own sentence: a language-aware wording
    // that stops saying the confident tier is shut would be a regression.
    assert!(
        reason.contains("capped below the confident tier"),
        "the reason says the confident tier is closed: {reason}"
    );
}

/// The reason distinguishes a load refusal from a grammar failure, because the
/// remedies differ (re-run the build vs. split or fix the file) and status
/// already says `budget_refused`.
#[test]
fn the_reason_says_budget_refused_only_for_a_budget_refusal() {
    let AnalysisStatus::Partial { reason } = status_of(&corpus(true)) else {
        panic!("a refused file must degrade the analysis");
    };
    assert!(
        reason.contains("1 of the parse failure(s) are budget-refused"),
        "{reason}"
    );

    let mut grammar_failure = corpus(false);
    let broken = grammar_failure.last_mut().unwrap();
    broken.parse_outcome = ParseOutcome::Failed {
        reason: "grammar fault".to_string(),
    };
    let AnalysisStatus::Partial { reason } = status_of(&grammar_failure) else {
        panic!("a failed file must degrade the analysis");
    };
    assert!(reason.contains("1 file(s) failed to parse"), "{reason}");
    assert!(!reason.contains("budget-refused"), "{reason}");
}

#[test]
fn no_finding_reaches_the_confident_tier_over_a_refused_file() {
    let reports = findings(&corpus(true));
    assert!(
        reports.iter().any(|report| report.symbol_name == "hidden"),
        "the premise: `hidden` (called only by the refused file) is a finding to cap: {reports:?}"
    );
    for report in &reports {
        assert!(
            report.confidence <= HIGHEST_DEGRADED_CONFIDENCE,
            "a finding over a corpus with a refused file reached the confident tier: {report:?}"
        );
    }
}

#[test]
fn the_same_corpus_without_the_refusal_is_ok_and_keeps_its_confident_tier() {
    let clean = corpus(false);
    assert!(matches!(status_of(&clean), AnalysisStatus::Ok));
    let reports = findings(&clean);
    assert!(
        !reports.iter().any(|report| report.symbol_name == "hidden"),
        "with the caller parsed, `hidden` is live: {reports:?}"
    );
    let orphan = reports
        .iter()
        .find(|report| report.symbol_name == "orphan")
        .unwrap_or_else(|| panic!("`orphan` must be a finding: {reports:?}"));
    assert!(
        orphan.confidence > HIGHEST_DEGRADED_CONFIDENCE,
        "an undegraded corpus must keep the confident tier, or the cap above proves nothing: \
         {orphan:?}"
    );
}
