//! The graded ceiling, measured at the only scale where it is graded.
//!
//! **The suite that shipped with the grading could not observe it.** Every
//! fixture in `extraction_coverage_liveness.rs` and `dead_clusters.rs` is a two-
//! to five-file corpus, so the blind share lands at 20–50% — past the 12.3%
//! crossover where `ExtractionCoverage::cap` is already clamped to
//! `COVERAGE_LOSS_CONFIDENCE_CAP`. Those tests therefore pass for the *old*
//! reason, and the entire curve between the floor and `HIGHEST_DEGRADED_
//! CONFIDENCE` was asserted by nothing. Four of them are named in the audit:
//!
//! * `a_call_blind_corpus_cannot_reach_the_confident_tier` — a **pure**
//!   Terraform corpus, 100% blind, so it exercises the floor and nothing else;
//! * `a_lost_caller_is_not_confident_evidence_of_death` — two files, 50% blind;
//! * `a_pattern_recovered_file_is_lost_call_coverage_even_though_it_parsed` —
//!   likewise;
//! * `coverage_loss_caps_a_cluster_finding` — two files, and at repository scale
//!   the cap it is named for does nothing at all for a small component.
//!
//! Verified rather than asserted: with the fix in this change applied and every
//! one of those tests left untouched, all 52 + 195 tests in this crate stayed
//! green while the headline number moved from 0.89 to 0.663. A suite that cannot
//! see a design change is a suite that will pass through the next one.
//!
//! So this file holds the repository-scale half — ≥100 files, one blind — and
//! asserts the **actual graded value** rather than a bound that the floor
//! satisfies for free.

use devmap_analyze::{
    analyze, analyze_liveness_with_coverage, dead_clusters, DeadSymbolReport, DiscoveryCoverage,
    ExtractionCoverage, CALL_BLIND_REASON, COVERAGE_LOSS_CONFIDENCE_CAP, COVERAGE_LOSS_REASON,
    HIGHEST_DEGRADED_CONFIDENCE,
};
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_resolve::Resolver;

/// The ceiling a corpus with one blind file in a hundred is charged at.
///
/// `MIN_CHARGED_BLIND_SHARE` is 5% and `COVERAGE_CEILING_EXPONENT` is 8, so
/// `0.95^8`. Written out rather than recomputed from the constants: a test that
/// derives its expectation from the same two dials it is checking would agree
/// with any value they took, which is how the older fixtures came to pass for
/// the wrong reason.
const GRADED_CEILING: f32 = 0.663_420_4;

/// What the ceiling was before the minimum charge, and must never be again.
///
/// The audit's Q-1 in a thousand-file repository: `0.999^8 = 0.992`, clamped to
/// `HIGHEST_DEGRADED_CONFIDENCE`. A claim whose caller demonstrably exists and
/// was never read, published one tier below the act-on threshold with ten
/// thousandths and a rounding rule between it and `extracted`.
const UNGRADED_CEILING: f32 = HIGHEST_DEGRADED_CONFIDENCE;

fn close(left: f32, right: f32) -> bool {
    (left - right).abs() < 1e-4
}

/// `lib.py` declares `helper` and `dangling`; `app.py` is `helper`'s only
/// caller. Borrowed verbatim from `extraction_coverage_liveness.rs` so the two
/// files cannot drift about what produces the finding.
fn q1_pair() -> Vec<Extraction> {
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

/// Clean Python files that call only themselves, to make the corpus a
/// repository rather than a pair.
fn filler(count: usize) -> Vec<Extraction> {
    (0..count)
        .map(|index| {
            extract_file(
                &format!("pkg/clean_{index}.py"),
                "def live():\n    return 1\n\n\ndef caller():\n    return live()\n",
            )
        })
        .collect()
}

/// What the extractor produces when a parse is refused: the `File` node only.
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

fn liveness(
    extractions: &[Extraction],
    discovery: DiscoveryCoverage,
) -> (Vec<DeadSymbolReport>, ExtractionCoverage) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let outcome = analyze_liveness_with_coverage(extractions, &resolution, discovery);
    (outcome.reports, outcome.coverage)
}

fn finding<'a>(reports: &'a [DeadSymbolReport], file: &str, symbol: &str) -> &'a DeadSymbolReport {
    reports
        .iter()
        .find(|report| report.file_path == file && report.symbol_name.ends_with(symbol))
        .unwrap_or_else(|| panic!("{file}::{symbol} must be reported: {reports:?}"))
}

// ---------------------------------------------------------------------------
// The lost caller, at scale
// ---------------------------------------------------------------------------

/// **R3.** Q-1 in a hundred-file repository must not publish at 0.89.
///
/// The two-file version of this fixture is 50% blind and lands on the floor,
/// which is why it never saw the grading. At repository scale the file-count
/// ratio is 1%, and before the minimum charge that bought a ceiling of 0.89 —
/// the top of `inferred`, for a claim the corpus itself contradicts.
#[test]
fn a_lost_caller_in_a_large_corpus_is_graded_not_nearly_confident() {
    let mut extractions = q1_pair();
    refuse(&mut extractions[1]);
    extractions.extend(filler(98));

    let (reports, coverage) = liveness(&extractions, DiscoveryCoverage::none());
    assert_eq!(coverage.parse_failed_files, 1);
    assert_eq!(
        coverage.files_with_call_extraction, 99,
        "the denominator must be the files whose calls were looked for: {coverage:?}"
    );

    let helper = finding(&reports, "lib.py", "helper");
    assert!(
        !helper.is_exempt,
        "hiding the finding would be its own lie: {helper:?}"
    );
    assert!(
        close(helper.confidence, GRADED_CEILING),
        "one blind file in a hundred must charge the minimum share, not the \
         file-count ratio: got {}, want {GRADED_CEILING}",
        helper.confidence
    );
    assert!(
        helper.confidence < UNGRADED_CEILING - 0.2,
        "and it must be nowhere near the pre-fix ceiling of {UNGRADED_CEILING}: {}",
        helper.confidence
    );
    assert_eq!(
        helper.exemption_reason.as_deref(),
        Some(COVERAGE_LOSS_REASON),
        "the finding must still say why it is not confident: {helper:?}"
    );
}

/// The same shape through discovery rather than through a parse failure — which
/// is the audit's actual Q-1: the caller sits in a file over `MAX_SOURCE_BYTES`
/// and discovery never handed it to an extractor at all.
#[test]
fn a_caller_discovery_never_read_is_graded_the_same_way() {
    // `app.py` is *absent* from the corpus, exactly as a refused file is, and
    // the refusal is carried in from discovery because a file with no
    // `Extraction` cannot be counted from the slice.
    let mut extractions = vec![q1_pair().remove(0)];
    extractions.extend(filler(99));

    let (reports, coverage) = liveness(&extractions, DiscoveryCoverage::refused(1));
    assert_eq!(coverage.discovery_refused_files, 1);
    assert_eq!(coverage.files_with_call_extraction, 100);

    let helper = finding(&reports, "lib.py", "helper");
    assert!(
        close(helper.confidence, GRADED_CEILING),
        "a refused file is charged the same minimum as any other hole: got {}",
        helper.confidence
    );
}

/// A pattern-recovered file at scale, the third of the four named fixtures.
#[test]
fn a_pattern_recovered_file_in_a_large_corpus_is_graded() {
    let mut extractions = q1_pair();
    extractions[1].parse_outcome = ParseOutcome::Fallback {
        reason: "no grammar linked".to_string(),
    };
    extractions[1].engine = ExtractionEngine::RegexFallback {
        requested_language: extractions[1].language.clone(),
    };
    extractions[1].calls.clear();
    extractions[1].imports.clear();
    extractions.extend(filler(98));

    let (reports, coverage) = liveness(&extractions, DiscoveryCoverage::none());
    assert_eq!(coverage.pattern_recovered_files, 1);
    let helper = finding(&reports, "lib.py", "helper");
    assert!(
        close(helper.confidence, GRADED_CEILING),
        "got {}",
        helper.confidence
    );
}

/// The grading still does its job: a clean corpus reaches `extracted`, and the
/// three tiers stay apart. The minimum charge must not have re-flattened what
/// the grading exists to separate.
#[test]
fn the_minimum_charge_keeps_the_ladder_it_was_added_to_protect() {
    let mut extractions = q1_pair();
    refuse(&mut extractions[1]);
    extractions.extend(filler(98));
    let (reports, _) = liveness(&extractions, DiscoveryCoverage::none());

    let confident = finding(&reports, "lib.py", "helper").confidence;
    let weaker: Vec<f32> = reports
        .iter()
        .filter(|report| !report.is_exempt && report.confidence < confident)
        .map(|report| report.confidence)
        .collect();
    assert!(
        confident > 0.5,
        "the confident tier must stay well above the ambiguous one: {confident}"
    );
    for value in &weaker {
        assert!(
            confident - value > 0.2,
            "the tiers must stay separated by more than a rounding step: \
             {confident} vs {value}"
        );
    }

    // And a complete corpus is untouched.
    let clean = q1_pair();
    let (clean_reports, clean_coverage) = liveness(&clean, DiscoveryCoverage::none());
    assert!(clean_coverage.is_complete(), "{clean_coverage:?}");
    assert!(
        close(
            finding(&clean_reports, "lib.py", "dangling").confidence,
            0.9
        ),
        "a clean corpus must still reach the confident tier"
    );
}

// ---------------------------------------------------------------------------
// A file that is itself blind
// ---------------------------------------------------------------------------

/// A clean parse in a language this build extracts no calls for, with symbols
/// that are **not** all exported.
///
/// Synthesised, and it has to be. The two call-blind grammars that are actually
/// linked cannot express this state: `hcl` marks every symbol `is_exported`
/// (`treesitter.rs`), so every HCL symbol is exempt before the ceiling is
/// reached, and `cfml` produces no declarations at all. `cobol` and `vb` are
/// unlinked. So the defect is real and latent — it becomes live the day a
/// call-blind grammar starts declaring an unexported symbol, which is a change
/// nobody would think to connect to a confidence ceiling in another crate.
///
/// So the fixture is a **real** Terraform parse — `language: "hcl"`,
/// `ExtractionEngine::TreeSitter`, `ParseOutcome::Clean`, no calls, which is
/// what `extract_file` genuinely produces — with one field flipped: its
/// declarations stop claiming to be exported. That is the smallest mutation
/// that reaches the state, and it names the cross-crate dependency exactly:
/// `treesitter.rs`'s `is_exported: true` for HCL is currently the only thing
/// standing between a call-blind symbol and this ceiling.
fn call_blind_file(path: &str) -> Extraction {
    let mut ext = extract_file(
        path,
        "module \"unreferenced\" {\n  source = \"./x\"\n}\n\n\
         module \"also_unreferenced\" {\n  source = \"./y\"\n}\n",
    );
    assert_eq!(
        ext.language, "hcl",
        "fixture assumption: {path} parses as HCL"
    );
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "fixture assumption: HCL parses cleanly — {:?}",
        ext.parse_outcome
    );
    assert!(
        ext.calls.is_empty(),
        "fixture assumption: this build extracts no HCL calls"
    );
    for symbol in ext.symbols.iter_mut() {
        if symbol.kind != SymbolKind::File {
            symbol.is_exported = false;
        }
    }
    assert!(
        ext.symbols
            .iter()
            .any(|s| s.kind != SymbolKind::File && !s.name.starts_with('_')),
        "the fixture must declare a reportable symbol, or the test is vacuous"
    );
    ext
}

/// **R4.** A wholly call-blind file must not take the corpus-wide ceiling.
///
/// Before the fix, `file_is_call_blind` selected only the *reason string*. So in
/// a 99-readable / 1-blind corpus a symbol whose own file contributed no call
/// edges published at 0.89 carrying "this language has no call extractor in this
/// build" — a permanent property of the build, priced as a 1% transient risk.
/// The correct local blind share for that file is 1.0.
#[test]
fn a_symbol_in_a_call_blind_file_is_capped_by_its_own_file_not_the_corpus() {
    let mut extractions = vec![call_blind_file("infra/main.tf")];
    extractions.extend(filler(99));

    let (reports, coverage) = liveness(&extractions, DiscoveryCoverage::none());
    assert_eq!(
        coverage.call_blind_files, 1,
        "one blind file in a hundred: {coverage:?}"
    );
    assert_eq!(coverage.files_with_call_extraction, 99);

    let blind = finding(&reports, "infra/main.tf", "module.unreferenced");
    assert!(!blind.is_exempt, "the finding stays visible: {blind:?}");
    assert!(
        close(blind.confidence, COVERAGE_LOSS_CONFIDENCE_CAP),
        "a file whose calls were never looked for is 100% blind for its own \
         symbols, whatever the rest of the corpus did: got {}, want \
         {COVERAGE_LOSS_CONFIDENCE_CAP}",
        blind.confidence
    );
    assert_eq!(
        blind.exemption_reason.as_deref(),
        Some(CALL_BLIND_REASON),
        "and it names its own blindness rather than the corpus's: {blind:?}"
    );
}

/// The other direction, in one corpus: a finding whose own file was read takes
/// the corpus ceiling, and a finding whose file was not takes the floor. The
/// two must not be the same number, or the local rule is decorative.
#[test]
fn one_corpus_prices_a_readable_file_and_a_blind_file_differently() {
    let mut extractions = q1_pair();
    extractions.push(call_blind_file("infra/main.tf"));
    extractions.extend(filler(97));
    // `helper` keeps its caller here, so the readable-file finding under test is
    // `dangling`, which nothing calls in any corpus.

    let (reports, coverage) = liveness(&extractions, DiscoveryCoverage::none());
    assert_eq!(coverage.call_blind_files, 1);

    let readable = finding(&reports, "lib.py", "dangling").confidence;
    let blind = finding(&reports, "infra/main.tf", "module.unreferenced").confidence;

    assert!(
        close(readable, GRADED_CEILING),
        "the readable file takes the corpus ceiling: {readable}"
    );
    assert!(
        close(blind, COVERAGE_LOSS_CONFIDENCE_CAP),
        "the blind file takes its own: {blind}"
    );
    assert!(
        readable - blind > 0.25,
        "the two must be visibly different claims: {readable} vs {blind}"
    );
}

// ---------------------------------------------------------------------------
// Clusters
// ---------------------------------------------------------------------------

/// Three functions that only call each other, reachable from nothing.
const ABANDONED: &str = "def alpha():\n    return beta()\n\n\n\
                         def beta():\n    return gamma()\n\n\n\
                         def gamma():\n    return alpha()\n";

/// A ring of `n` mutually recursive functions, so the component's size is a
/// parameter of the test rather than a constant of the fixture.
fn ring(n: usize) -> String {
    let mut source = String::new();
    for index in 0..n {
        source.push_str(&format!(
            "def f{index}():\n    return f{next}()\n\n\n",
            next = (index + 1) % n
        ));
    }
    source
}

/// **R6, fourth fixture.** `coverage_loss_caps_a_cluster_finding` is a two-file
/// corpus, and its name claims more than the realistic case delivers.
///
/// At repository scale the cluster ceiling for a *small* component genuinely
/// does not bite: `0.95^11 = 0.569`, and `DEAD_CLUSTER_CONFIDENCE` is 0.5, so
/// `cap_cluster(0.5, 3)` returns 0.5 unchanged. That is not a defect — a
/// three-member component in a corpus that is 95% read is a reasonably strong
/// claim — but it is the opposite of what the two-file test's name asserts, and
/// leaving it unstated is how a cap comes to be believed in where it is inert.
///
/// So the property is measured rather than assumed: the compounding bites from
/// the member count where it mathematically must, and not before.
#[test]
fn the_cluster_ceiling_bites_at_the_size_where_compounding_passes_the_verdict() {
    let coverage = ExtractionCoverage {
        call_blind_files: 1,
        files_with_call_extraction: 99,
        ..ExtractionCoverage::default()
    };
    assert!(!coverage.is_complete());

    // `0.95^(8 + m) < 0.5` first holds at m = 6.
    let mut first_capped = None;
    for size in 1usize..=64 {
        let capped = coverage.cap_cluster(0.5, size);
        if capped < 0.5 - 1e-6 && first_capped.is_none() {
            first_capped = Some(size);
        }
        if let Some(at) = first_capped {
            assert!(
                size < at || capped < 0.5,
                "once the ceiling passes the verdict it must stay past it \
                 (size={size}, capped={capped})"
            );
        }
    }
    assert_eq!(
        first_capped,
        Some(6),
        "the compounding must start biting where the arithmetic says it does; \
         a change to the exponent, the floor or the minimum charge moves this \
         number and should have to come here and say so"
    );

    // And a large component in the same corpus is capped hard, which is the
    // claim the cluster ceiling actually exists to make.
    assert!(
        coverage.cap_cluster(0.5, 40) <= COVERAGE_LOSS_CONFIDENCE_CAP + 1e-6,
        "a forty-member component in a degraded corpus is a weak claim: {}",
        coverage.cap_cluster(0.5, 40)
    );
}

/// End to end, at scale: a large abandoned component in a corpus with one blind
/// file is published at the floor, and the same component in a clean corpus is
/// not.
///
/// The OFF direction is the half that matters. A cap asserted only in the
/// degraded case passes against code that caps unconditionally.
#[test]
fn a_large_cluster_is_capped_only_when_the_corpus_has_a_hole() {
    let build = |blind: bool| {
        let mut extractions = vec![extract_file("ring.py", &ring(12))];
        extractions.extend(filler(99));
        if blind {
            extractions.push(call_blind_file("infra/main.tf"));
        }
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        analyze(&extractions, &resolution)
    };

    let clean = build(false);
    assert!(
        !clean.dead_clusters.clusters.is_empty(),
        "the ring must be found at all: {:?}",
        clean.dead_clusters
    );
    let clean_confidence = clean.dead_clusters.clusters[0].confidence;
    assert!(
        clean_confidence > COVERAGE_LOSS_CONFIDENCE_CAP,
        "a complete corpus must not be capped: {clean_confidence}"
    );

    let degraded = build(true);
    assert!(!degraded.dead_clusters.clusters.is_empty());
    for cluster in &degraded.dead_clusters.clusters {
        // `0.95^(8 + 12) = 0.358`, so the compounding takes a twelve-member
        // component from `inferred` down into `ambiguous` without reaching the
        // floor. Asserted as the tier boundary rather than as the floor: the
        // floor would be satisfied by a cap that ignored the size entirely,
        // which is the very thing `cap_cluster` exists not to be.
        assert!(
            cluster.confidence < 0.4,
            "a twelve-member component in a degraded corpus must fall out of \
             the `inferred` tier even at repository scale: {cluster:?}"
        );
        assert!(
            cluster.confidence < clean_confidence - 0.1,
            "and it must be a visibly weaker claim than the same component in a \
             complete corpus ({clean_confidence}): {cluster:?}"
        );
        assert!(
            cluster.confidence >= COVERAGE_LOSS_CONFIDENCE_CAP,
            "while staying inside the declared band: {cluster:?}"
        );
    }
}

/// The cluster pass and the single-symbol pass must agree about what the corpus
/// is, at scale as well as in a two-file fixture.
#[test]
fn cluster_and_symbol_ceilings_read_the_same_corpus() {
    let mut extractions = vec![extract_file("ring.py", ABANDONED)];
    extractions.extend(filler(99));
    extractions.push(call_blind_file("infra/main.tf"));

    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let scan = dead_clusters(&extractions, &resolution);
    assert!(!scan.clusters.is_empty(), "{scan:?}");

    let summary = analyze(&extractions, &resolution);
    let coverage = ExtractionCoverage {
        call_blind_files: 1,
        files_with_call_extraction: 100,
        ..ExtractionCoverage::default()
    };
    for cluster in &summary.dead_clusters.clusters {
        let expected = coverage.cap_cluster(scan.clusters[0].confidence, cluster.size);
        assert!(
            close(cluster.confidence, expected),
            "the published cluster confidence must be the ceiling the same \
             coverage record computes: got {}, want {expected}",
            cluster.confidence
        );
    }
}
