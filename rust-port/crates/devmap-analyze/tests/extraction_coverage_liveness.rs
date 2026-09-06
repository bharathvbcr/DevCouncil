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

    let coverage = extraction_coverage(&extractions);
    assert_eq!(
        ExtractionCoverage {
            files_with_call_extraction: coverage.files_with_call_extraction,
            ..ExtractionCoverage::default()
        },
        coverage,
        "a file no grammar was ever expected for is not a gap in coverage"
    );
    // Prose sits in **neither** side of the ratio, which is the stronger claim
    // and the one that matters now that `cap()` grades on it. In the numerator
    // it would mark every repository degraded; in the denominator it would
    // dilute the blind share with files nobody ever asked a grammar to read, so
    // adding a README to a broken repository would raise its confidence.
    //
    // Written as a whole-struct comparison with the denominator threaded
    // through so a *new* gap counter still fails this test, and then pinned
    // exactly on the line below.
    assert_eq!(
        coverage.files_with_call_extraction, 2,
        "the two Python files are the corpus; the README is not part of the \
         question: {coverage:?}"
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

// ---------------------------------------------------------------------------
// W0.2 — a clean parse in a language with no call extractor is a coverage hole
// ---------------------------------------------------------------------------
//
// The three gaps above all begin with a failure: a parse that did not finish, a
// grammar that was not linked, a walk that refused a file. CFML and Terraform
// fail at none of them. They parse `Clean`, `is_parse_failure` is false,
// `Fallback` never matches, and no arm of `extract_node` or `langcalls` pushes
// a call for either language — so the file contributed zero call edges and
// `is_complete()` stayed true.
//
// Measured against the pre-change kernel, `probe.cfm`'s and `probe.tf`'s
// symbols were reported at confidence 0.9 with `exemption_reason: None`, which
// `confidence_label` renders `extracted` — the tier whose contract is "safe to
// act on". That is the same maximum-confidence-from-a-check-that-did-not-run
// shape as Q-1, arriving through a door the coverage machinery had no way to
// see.

/// Terraform: parses `Clean`, extracts references, extracts no calls.
const TERRAFORM: &str = r#"module "helper" {
  source = "./helper"
}

resource "aws_s3_bucket" "b" {
  bucket = lower(var.name)
}
"#;

/// CFML: parses `Clean` and extracts nothing at all.
const CFML: &str = r#"<cfscript>
    component Widget {
        function render() {
            return help(this.name);
        }
    }
</cfscript>
"#;

/// The language really is call-blind, and really does parse cleanly.
///
/// Stated separately so that if a CFML or HCL call extractor ever lands, this
/// is the test that fails first and explains why the ones below changed —
/// rather than leaving a reader to guess whether the fixture or the kernel
/// moved.
#[test]
fn the_call_blind_fixtures_parse_clean_and_extract_no_calls() {
    for (path, source) in [("main.tf", TERRAFORM), ("Widget.cfm", CFML)] {
        let extraction = extract_file(path, source);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{path} must parse cleanly, or it would be charged as a parse \
             failure and this whole class of hole would already be covered: {:?}",
            extraction.parse_outcome
        );
        assert!(
            !extraction.is_parse_failure(),
            "{path} must not read as a parse failure"
        );
        assert!(
            extraction.calls.is_empty(),
            "{path} is expected to extract no calls"
        );
    }

    // Terraform declares symbols; CFML declares none at all. Asserted apart
    // rather than with a shared `> 1`, because the two are blind in different
    // amounts and a test that hid the difference would let CFML's extractor
    // start producing symbols without anyone noticing that it now has
    // something to report dead.
    assert!(
        extract_file("main.tf", TERRAFORM).symbols.len() > 1,
        "Terraform must declare something, or `a_call_blind_corpus_cannot_\
         reach_the_confident_tier` holds vacuously"
    );
    assert_eq!(
        extract_file("Widget.cfm", CFML)
            .symbols
            .iter()
            .filter(|s| s.kind != SymbolKind::File)
            .count(),
        0,
        "CFML extracts no declarations at all; if that changes, its symbols \
         become reportable and the call-blind cap is what stands between them \
         and the `extracted` tier"
    );
}

/// A pure-Terraform corpus yields no `extracted`-tier findings, and says why.
#[test]
fn a_call_blind_corpus_cannot_reach_the_confident_tier() {
    let extractions = vec![extract_file("main.tf", TERRAFORM)];
    let summary = summarize(&extractions);

    let reason = match &summary.status {
        AnalysisStatus::Partial { reason } => reason.clone(),
        other => panic!(
            "a corpus whose only language has no call extractor is not a complete scan: {other:?}"
        ),
    };
    assert!(
        reason.contains("no call extractor at all"),
        "the status must name the blindness as permanent, not as a transient \
         coverage loss: {reason}"
    );

    for report in summary.dead_symbols.iter().filter(|r| !r.is_exempt) {
        assert!(
            report.confidence <= COVERAGE_LOSS_CONFIDENCE_CAP,
            "an `extracted`-tier finding in a call-blind file: {report:?}"
        );
        assert_eq!(
            report.exemption_reason.as_deref(),
            Some(CALL_BLIND_REASON),
            "a call-blind finding must name its own blindness, not the \
             corpus-level coverage loss: {report:?}"
        );
    }
}

/// The same, for CFML, plus the counter.
#[test]
fn a_cfml_file_is_charged_as_call_blind() {
    let extractions = vec![extract_file("Widget.cfm", CFML)];
    let outcome = {
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        analyze_liveness_with_coverage(&extractions, &resolution, DiscoveryCoverage::none())
    };

    assert_eq!(outcome.coverage.call_blind_files, 1);
    assert_eq!(
        outcome.coverage.import_blind_files, 0,
        "CFML used to be both. W0.3 move 2 reads `template=\"…\"`, so its \
         imports are no longer missing — its *calls* still are, and the two \
         are charged separately precisely so one can be fixed without the \
         other silently going with it"
    );
    assert_eq!(
        outcome.coverage.parse_failed_files, 0,
        "a clean parse must not be charged as a failure; the two are different \
         facts and a reader acts on them differently"
    );
    assert!(!outcome.coverage.is_complete());
    assert_eq!(outcome.coverage.files_without_call_extraction(), 1);
}

/// The OFF direction, and the one that keeps the gate from swallowing the tree.
///
/// Prose and data formats declare no capabilities either, and they outnumber
/// source files in most repositories — 294 of this one's 1,310. If the
/// call-blind charge were driven by capability alone rather than by "a grammar
/// actually read this file", `is_complete()` would be false on every corpus in
/// existence and the cap would fire on every finding, which is indistinguishable
/// from having deleted dead-code detection.
#[test]
fn prose_and_data_files_are_not_call_blind() {
    let mut extractions = fixture();
    extractions.push(extract_file("README.md", "# Title\n\nProse.\n"));
    extractions.push(extract_file("package-lock.json", "{\"a\": 1}\n"));
    extractions.push(extract_file("ci.yaml", "steps:\n  - run: make\n"));

    let outcome = {
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        analyze_liveness_with_coverage(&extractions, &resolution, DiscoveryCoverage::none())
    };

    assert_eq!(
        outcome.coverage.call_blind_files, 0,
        "a `.md` is not a call-extraction hole; no grammar was ever wanted for it"
    );
    assert_eq!(outcome.coverage.import_blind_files, 0);
    assert!(
        outcome.coverage.is_complete(),
        "a Python corpus with a README must still report a complete scan: {:?}",
        outcome.coverage
    );

    let dangling = outcome
        .reports
        .iter()
        .find(|r| r.file_path == "lib.py" && r.symbol_name == "dangling")
        .expect("the uncalled symbol must still be reported");
    assert_eq!(
        dangling.confidence, 0.9,
        "adding a README must not demote a finding about Python"
    );
}

/// A pattern-recovered file is charged once, not twice.
///
/// `.ps1` reaches `RegexFallback`, which extracts no calls and no imports — so
/// it satisfies the call-blind predicate on capability alone. It is already
/// charged as `PatternRecovered`, and charging it again would report two holes
/// where the file has one, inflating `files_without_call_extraction()` past the
/// number of files that actually have a hole.
#[test]
fn a_pattern_recovered_file_is_not_also_charged_as_call_blind() {
    let extractions = vec![extract_file(
        "build.ps1",
        "function Render {\n  Help $args\n}\nRender\n",
    )];
    let outcome = {
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        analyze_liveness_with_coverage(&extractions, &resolution, DiscoveryCoverage::none())
    };

    assert_eq!(outcome.coverage.pattern_recovered_files, 1);
    assert_eq!(
        outcome.coverage.call_blind_files, 0,
        "double-charged: one file, two holes reported"
    );
    assert_eq!(outcome.coverage.files_without_call_extraction(), 1);
}

/// Import-blindness must not demote a dead-code verdict.
///
/// A C# corpus is import-blind — `using System;` names a namespace, not a
/// file, so there is nothing for an extractor to resolve and W0.3 move 2
/// declined the language on purpose — but its calls are extracted normally,
/// and the dead-symbol verdict rests on call edges. Folding import-blindness
/// into `is_complete()` would cap every finding in every such repository at
/// `ambiguous`, for a blindness the verdict does not rest on. It is charged,
/// published, and consumed by `unwired_candidates` instead.
///
/// This test was written against Java, which was import-blind until W0.3 move
/// 2 and is not any more. Repointing it at C# rather than deleting it keeps the
/// invariant under test — the two blindnesses stay independent — and C# is one
/// of the four languages whose exclusion is a documented decision rather than a
/// gap waiting to close, so the fixture will not rot the same way.
#[test]
fn import_blindness_is_charged_without_capping_the_call_verdict() {
    let extractions = vec![extract_file(
        "A.cs",
        "using System;\n\nclass A {\n  private void Used() {}\n  \
         private void Unused() {}\n  private void Run() { Used(); }\n}\n",
    )];
    let outcome = {
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        analyze_liveness_with_coverage(&extractions, &resolution, DiscoveryCoverage::none())
    };

    assert_eq!(outcome.coverage.import_blind_files, 1);
    assert_eq!(
        outcome.coverage.call_blind_files, 0,
        "C# extracts calls; only its imports are missing"
    );
    assert!(
        outcome.coverage.is_complete(),
        "import blindness is not a call-coverage hole: {:?}",
        outcome.coverage
    );
    assert!(
        outcome
            .reports
            .iter()
            .any(|r| r.symbol_name.contains("Unused")
                && !r.is_exempt
                && r.confidence > COVERAGE_LOSS_CONFIDENCE_CAP),
        "a genuinely uncalled C# method must still reach the confident tier: {:?}",
        outcome.reports
    );
}
