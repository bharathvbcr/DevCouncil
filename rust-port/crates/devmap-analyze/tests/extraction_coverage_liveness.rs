//! "Nothing calls it" is only evidence when calls were looked for — corpus-wide.
//!
//! `analyze_liveness` already applied that sentence to a parse-failed file's
//! *own* symbols (X6). The cross-file half was missing: a file that contributed
//! no call edges was also the only possible caller of somebody else's symbol,
//! and nothing downgraded findings about the files those lost edges pointed at.
//!
//! Measured before the fix, from the audit's fixture reproduced verbatim in
//! `a_lost_caller_is_not_confident_evidence_of_death` below: `lib.py::helper`
//! — called from `app.py` and from nowhere else — was reported at confidence
//! 0.9, which `code_graph.rs::confidence_label` renders `extracted`, the tier
//! `CLAUDE.md` tells agents to act on. A check that could not run produced a
//! maximum-confidence proposal to delete working code.
//!
//! Every test here asserts **both** directions. A degraded flag that is always
//! on is worth exactly as little as one that is always off, and a suite that
//! only pins the degraded case passes against code that reports degraded
//! unconditionally.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::Resolver;

/// `lib.py` declares `helper` and `dangling`; `app.py` imports and calls
/// `helper` and nothing else. `app.py` is `helper`'s only caller.
fn fixture() -> Vec<Extraction> {
    vec![
        extract_file(
            "lib.py",
            "def helper():\n    return 1\n\n\ndef dangling():\n    return 2\n",
        ),
        extract_file(
            "app.py",
            "from lib import helper\n\n\ndef main():\n    return helper()\n",
        ),
    ]
}

/// Turn an extraction into what the extractor produces when a parse is refused.
///
/// Shaped exactly as `refused_extraction` builds one (`treesitter.rs`): engine
/// `Unavailable`, the `File` node only, no imports, no calls, no wiring. Simply
/// overwriting `parse_outcome` and leaving the extracted calls in place would
/// be a fixture that cannot reproduce the defect at all — the resolver still
/// emits the call edge and `helper` still reports live.
fn refuse(ext: &mut Extraction) {
    ext.parse_outcome = ParseOutcome::Failed {
        reason: "forced parse failure".to_string(),
    };
    ext.engine = ExtractionEngine::Unavailable {
        requested_language: ext.language.clone(),
    };
    ext.symbols.retain(|sym| sym.kind == SymbolKind::File);
    ext.imports.clear();
    ext.calls.clear();
    ext.wiring.clear();
}

fn summarize(extractions: &[Extraction]) -> AnalysisSummary {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    analyze(extractions, &resolution)
}

fn finding<'a>(
    summary: &'a AnalysisSummary,
    file: &str,
    symbol: &str,
) -> Option<&'a DeadSymbolReport> {
    summary
        .dead_symbols
        .iter()
        .find(|report| report.file_path == file && report.symbol_name == symbol)
}

/// The OFF direction, and the one that makes every other test here mean
/// something: a corpus the extractor read completely reports `Ok` and keeps
/// the confident tier.
#[test]
fn a_fully_read_corpus_reports_ok_and_keeps_the_confident_tier() {
    let extractions = fixture();
    let summary = summarize(&extractions);

    assert!(
        matches!(summary.status, AnalysisStatus::Ok),
        "nothing failed to parse, so nothing may degrade the status: {:?}",
        summary.status
    );
    assert!(
        finding(&summary, "lib.py", "helper").is_none_or(|r| r.is_exempt),
        "`helper` is called from app.py: {:?}",
        summary.dead_symbols
    );

    let dangling = finding(&summary, "lib.py", "dangling")
        .expect("a symbol nothing calls must still be reported");
    assert!(!dangling.is_exempt);
    assert_eq!(
        dangling.confidence, 0.9,
        "a complete scan must still reach the confident tier, or the cap has \
         replaced dead-code detection rather than qualified it"
    );
    assert_eq!(
        dangling.exemption_reason, None,
        "a complete scan carries no coverage caveat: {dangling:?}"
    );
}

/// The audit's Q-1 fixture, verbatim.
///
/// Before the fix this produced `confidence: 0.9`, `exemption_reason: None`,
/// and `AnalysisStatus::Ok`.
#[test]
fn a_lost_caller_is_not_confident_evidence_of_death() {
    let mut extractions = fixture();
    refuse(&mut extractions[1]);
    let summary = summarize(&extractions);

    let reason = match &summary.status {
        AnalysisStatus::Partial { reason } => reason.clone(),
        other => panic!("a corpus with an unread file is not `ok`: {other:?}"),
    };
    assert!(
        reason.contains("1 file(s) failed to parse"),
        "the reason must size the hole, not merely assert one: {reason}"
    );

    let helper = finding(&summary, "lib.py", "helper")
        .expect("the lost edge makes `helper` look uncalled — it must still be listed");
    assert!(
        !helper.is_exempt,
        "hiding the finding would be its own lie: {helper:?}"
    );
    assert!(
        helper.confidence <= COVERAGE_LOSS_CONFIDENCE_CAP,
        "a check that could not run must not reach the confident tier: {helper:?}"
    );
    assert_eq!(
        helper.exemption_reason.as_deref(),
        Some(COVERAGE_LOSS_REASON),
        "the finding must say why it is not confident: {helper:?}"
    );

    // The cap is a ceiling over the whole non-exempt population, not a rule
    // about the one symbol whose caller was lost — nothing can tell which
    // findings the missing edges would have contradicted.
    let dangling = finding(&summary, "lib.py", "dangling").expect("still reported");
    assert!(dangling.confidence <= COVERAGE_LOSS_CONFIDENCE_CAP);
}

/// The over-fire guard, and the reason this counts through
/// `Extraction::is_parse_failure` rather than a bare `matches!`.
///
/// Markdown, JSON, YAML, config and HTML all report `ParseOutcome::Failed` for
/// want of a grammar that does not exist and never will — 294 of this
/// repository's 1,310 files. Counting those would mark every build of every
/// real repository degraded and cap every dead-code finding in it, which is a
/// flag carrying no information.
#[test]
fn a_prose_file_with_no_grammar_is_not_lost_coverage() {
    let mut extractions = fixture();
    let readme = extract_file("README.md", "# Title\n\nSome prose.\n");
    assert!(
        matches!(readme.parse_outcome, ParseOutcome::Failed { .. }),
        "fixture assumption: prose reports Failed — {:?}",
        readme.parse_outcome
    );
    assert!(
        matches!(readme.engine, ExtractionEngine::NotApplicable { .. }),
        "fixture assumption: prose records NotApplicable — {:?}",
        readme.engine
    );
    extractions.push(readme);

    assert_eq!(
        extraction_coverage(&extractions),
        ExtractionCoverage::default(),
        "a file no grammar was ever expected for is not a gap in coverage"
    );

    let summary = summarize(&extractions);
    assert!(
        matches!(summary.status, AnalysisStatus::Ok),
        "a README must not degrade the graph: {:?}",
        summary.status
    );
    assert_eq!(
        finding(&summary, "lib.py", "dangling")
            .expect("still reported")
            .confidence,
        0.9,
        "and must not cap the confident tier"
    );
}

/// A pattern-recovered file loses call coverage too, and is counted apart.
///
/// `Fallback` extracts names and spans but, by construction, no calls and no
/// imports — so a `.proto` or `.ps1` calling a symbol contributes no evidence
/// that it is alive. Distinct from `Failed` because the two are different
/// claims and a reader deciding whether to act wants to know which.
#[test]
fn a_pattern_recovered_file_is_lost_call_coverage_and_is_counted_apart() {
    let mut extractions = fixture();
    extractions[1].parse_outcome = ParseOutcome::Fallback {
        reason: "no linked tree-sitter grammar".to_string(),
    };
    extractions[1].engine = ExtractionEngine::RegexFallback {
        requested_language: "python".to_string(),
    };
    extractions[1].imports.clear();
    extractions[1].calls.clear();

    let coverage = extraction_coverage(&extractions);
    assert_eq!(coverage.parse_failed_files, 0);
    assert_eq!(coverage.pattern_recovered_files, 1);
    assert_eq!(coverage.files_without_call_extraction(), 1);
    assert!(!coverage.is_complete());

    let summary = summarize(&extractions);
    let reason = match &summary.status {
        AnalysisStatus::Partial { reason } => reason.clone(),
        other => panic!("pattern recovery is not a complete scan: {other:?}"),
    };
    assert!(
        reason.contains("1 recovered by pattern"),
        "the two kinds of blindness must be reported apart: {reason}"
    );
    assert!(
        finding(&summary, "lib.py", "helper")
            .expect("listed")
            .confidence
            <= COVERAGE_LOSS_CONFIDENCE_CAP
    );
}

/// Both entry points report the same tiers.
///
/// `analyze_liveness` is a thin delegate over `analyze_liveness_with_coverage`;
/// if the cap moved into `analyze()` instead, a caller of the older function
/// would still be handed 0.9 findings from a corpus with a hole in it.
#[test]
fn both_liveness_entry_points_apply_the_same_cap() {
    let mut extractions = fixture();
    refuse(&mut extractions[1]);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let bare = analyze_liveness(&extractions, &resolution);
    let full = analyze_liveness_with_coverage(&extractions, &resolution, DiscoveryCoverage::none());

    assert_eq!(full.coverage.parse_failed_files, 1);
    assert_eq!(bare.len(), full.reports.len());
    for (left, right) in bare.iter().zip(full.reports.iter()) {
        assert_eq!(left.confidence, right.confidence);
        assert_eq!(left.symbol_name, right.symbol_name);
    }
    assert!(bare
        .iter()
        .filter(|report| !report.is_exempt)
        .all(|report| report.confidence <= COVERAGE_LOSS_CONFIDENCE_CAP));
}
