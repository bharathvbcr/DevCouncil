//! One unreadable file must not flatten the whole confidence ladder.
//!
//! `ExtractionCoverage::cap` was a *binary* gate: `is_complete()` asks whether
//! any of four counters is non-zero, and if one is, every non-exempt finding in
//! the generation is slammed to `COVERAGE_LOSS_CONFIDENCE_CAP`. The four
//! counters are corpus-wide, so one oversized vendored file, one `.proto`, one
//! `.tf` fixture — any of them — takes the whole repository down.
//!
//! Measured on this repository at the time this test was written: 1,502 files
//! indexed, of which **10** were blind (3 parse failures, 2 pattern-recovered,
//! 1 refused by discovery, 4 in a call-blind language). 0.67% of the corpus.
//! All **214** dead findings came back at exactly 0.35, and
//! `generation_dead_symbols` is read `ORDER BY confidence DESC, file_path` — so
//! with every confidence tied, the ranked list an agent reads was ordered
//! **alphabetically by path**, and the default 2,000-token budget showed it the
//! first ~66 filenames rather than the strongest evidence.
//!
//! Three claims collapsed into one number:
//!
//! * "no edge in the generation names this symbol" (0.9),
//! * "something calls it and the resolver could not say which" (0.4),
//! * "an unresolved call site names it" (0.4).
//!
//! The first is evidence *for* death and the other two are evidence *against*
//! it, and after the cap they printed identically. That is not conservatism —
//! conservatism keeps the ordering and lowers the ceiling. It is the loss of
//! the ladder the kernel spends its whole design budget building.
//!
//! What must **not** change, and is asserted below: an incomplete scan still
//! never reaches the `extracted` tier, and a corpus that is mostly blind still
//! bottoms out at the same 0.35 it does today. The floor is kept; only the
//! grading between the floor and `extracted` is new.

use devmap_analyze::{
    analyze_liveness_with_coverage, DeadSymbolReport, DiscoveryCoverage, ExtractionCoverage,
    COVERAGE_LOSS_CONFIDENCE_CAP, HIGHEST_DEGRADED_CONFIDENCE,
};
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

/// Two same-named extension functions and one call through an untypeable
/// receiver: the resolver names one candidate and cannot rule out the other, so
/// both land at `only_ambiguous_callers`. `neverCalled` in the same file is
/// reached by nothing at all and lands at the confident tier.
///
/// Borrowed verbatim from `receiver_qualified_liveness.rs` so the two tests
/// cannot drift about what produces which tier.
const AMBIGUOUS: &str = concat!(
    "class Task\n",
    "class Note\n",
    "private fun Task.toJson(): String = \"t\"\n",
    "private fun Note.toJson(): String = \"n\"\n",
    "private fun encode(tasks: List<Task>, notes: List<Note>): String {\n",
    "    return tasks.map { it.toJson() }.toString() + notes.map { it.toJson() }.toString()\n",
    "}\n",
    "private fun neverCalled(): String = \"x\"\n",
);

/// Parses `Clean`, declares symbols, and has no call extractor in this build —
/// so it is charged as `call_blind` and nothing about it is a *failure*.
const TERRAFORM: &str = "module \"helper\" {\n  source = \"./helper\"\n}\n";

/// `filler` clean Python files, so the blind share is a measured fraction
/// rather than "one of two".
fn corpus(filler: usize, blind_files: usize) -> Vec<Extraction> {
    let mut extractions = vec![extract_file("Codec.kt", AMBIGUOUS)];
    for index in 0..filler {
        extractions.push(extract_file(
            &format!("pkg/clean_{index}.py"),
            "def live():\n    return 1\n\n\ndef caller():\n    return live()\n",
        ));
    }
    for index in 0..blind_files {
        extractions.push(extract_file(&format!("infra/mod_{index}.tf"), TERRAFORM));
    }
    extractions
}

fn reports(extractions: &[Extraction]) -> (Vec<DeadSymbolReport>, ExtractionCoverage) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let outcome = analyze_liveness_with_coverage(extractions, &resolution, DiscoveryCoverage::none());
    (outcome.reports, outcome.coverage)
}

fn confidence_of(reports: &[DeadSymbolReport], symbol: &str) -> f32 {
    reports
        .iter()
        .find(|report| report.symbol_name == symbol && !report.is_exempt)
        .unwrap_or_else(|| panic!("{symbol} must be reported as a non-exempt finding: {reports:?}"))
        .confidence
}

/// The headline. One blind file in a hundred must not make "nothing calls this"
/// and "something calls this and we cannot say what" the same answer.
#[test]
fn a_small_coverage_hole_does_not_flatten_the_ladder() {
    let extractions = corpus(99, 1);
    let (reports, coverage) = reports(&extractions);

    assert_eq!(coverage.call_blind_files, 1, "one Terraform file is blind");
    assert!(
        !coverage.is_complete(),
        "the corpus is incomplete, or the cap under test never fires"
    );

    let confident = confidence_of(&reports, "neverCalled");
    let ambiguous = confidence_of(&reports, "Task.toJson");

    assert!(
        confident > ambiguous,
        "`neverCalled` (nothing calls it) and `Task.toJson` (something calls it \
         and the resolver could not say which) are opposite evidence and must \
         not rank equal: {confident} vs {ambiguous}"
    );
}

/// The honesty rule the grading must not buy its way past.
#[test]
fn an_incomplete_scan_still_never_reaches_the_extracted_tier() {
    // A single blind file among ten thousand — the most flattering ratio a real
    // repository could produce.
    let mut extractions = corpus(2_000, 1);
    extractions.push(extract_file("infra/one.tf", TERRAFORM));
    let (reports, coverage) = reports(&extractions);
    assert!(!coverage.is_complete());

    for report in reports.iter().filter(|report| !report.is_exempt) {
        assert!(
            report.confidence <= HIGHEST_DEGRADED_CONFIDENCE,
            "a finding from a scan with a hole in it must stay below the \
             `extracted` floor whatever the size of the hole: {report:?}"
        );
        // 0.9 is `EXTRACTED_FLOOR_MILLIS`; the ceiling has to sit under it, not
        // merely at it, or `confidence_label` renders `extracted`.
        assert!(
            report.confidence < 0.9,
            "the ceiling must be strictly below the extracted floor: {report:?}"
        );
    }
}

/// The floor is kept. A corpus that is mostly blind behaves exactly as it does
/// today, so the existing constant keeps its meaning rather than becoming
/// decorative.
#[test]
fn a_mostly_blind_corpus_still_bottoms_out_at_the_existing_cap() {
    // One readable file, ninety-nine blind ones.
    let extractions = corpus(0, 99);
    let (reports, coverage) = reports(&extractions);
    assert_eq!(coverage.call_blind_files, 99);

    let confident = confidence_of(&reports, "neverCalled");
    assert!(
        (confident - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "a corpus this blind must land on the existing floor, not below it and \
         not above it: {confident}"
    );
}

/// A complete corpus is untouched: the grading applies to the degraded path
/// only, and a clean repository must keep reaching `extracted`.
#[test]
fn a_complete_corpus_is_not_graded_at_all() {
    let extractions = corpus(20, 0);
    let (reports, coverage) = reports(&extractions);
    assert!(
        coverage.is_complete(),
        "no blind file was added, so coverage must be complete: {coverage:?}"
    );
    let confident = confidence_of(&reports, "neverCalled");
    assert!(
        (confident - 0.9).abs() < 1e-6,
        "a clean corpus must still reach the confident tier: {confident}"
    );
}

/// The cap is monotone. A stronger claim must never come back weaker than a
/// weaker one, at any blind share — which is the property whose absence is the
/// whole defect.
#[test]
fn the_cap_is_monotone_at_every_blind_share() {
    for blind in [0usize, 1, 2, 5, 25, 50, 99, 200] {
        let mut coverage = ExtractionCoverage::default();
        coverage.call_blind_files = blind;
        coverage.files_with_call_extraction = 100;
        let mut previous = f32::NEG_INFINITY;
        for input in [0.3f32, 0.4, 0.5, 0.9] {
            let got = coverage.cap(input);
            assert!(
                got >= previous,
                "cap must be non-decreasing in its input (blind={blind}, \
                 input={input}, got={got}, previous={previous})"
            );
            assert!(
                got <= input,
                "cap must never raise a confidence (blind={blind}, \
                 input={input}, got={got})"
            );
            previous = got;
        }
    }
}

/// The ceiling falls as the hole grows, and never rises above it. Stated as a
/// sweep rather than three examples so a future formula cannot satisfy the
/// endpoints and misbehave between them.
#[test]
fn the_ceiling_falls_as_the_corpus_goes_blind() {
    let mut previous = f32::INFINITY;
    let mut seen: Vec<f32> = Vec::new();
    for blind in [1usize, 2, 5, 10, 25, 50, 100, 200, 400] {
        let mut coverage = ExtractionCoverage::default();
        coverage.call_blind_files = blind;
        coverage.files_with_call_extraction = 100;
        let got = coverage.cap(0.9);
        assert!(
            got <= previous,
            "a larger hole must not buy a higher ceiling (blind={blind}, \
             got={got}, previous={previous})"
        );
        assert!(
            (COVERAGE_LOSS_CONFIDENCE_CAP..=HIGHEST_DEGRADED_CONFIDENCE).contains(&got),
            "the ceiling must stay inside its declared band \
             (blind={blind}, got={got})"
        );
        previous = got;
        seen.push(got);
    }
    // The assertions above hold vacuously for a constant, which is exactly what
    // the pre-fix cap was — so without this the sweep would pass against the
    // defect it exists to fence. A ceiling that does not move with the hole is
    // not a graded ceiling.
    let distinct = seen.iter().fold(Vec::new(), |mut acc: Vec<f32>, value| {
        if !acc.iter().any(|seen| (seen - value).abs() < 1e-6) {
            acc.push(*value);
        }
        acc
    });
    assert!(
        distinct.len() >= 3,
        "the ceiling must actually respond to the size of the hole, not merely \
         fail to rise: {seen:?}"
    );
}

/// The crossover, pinned as a number.
///
/// `COVERAGE_CEILING_EXPONENT` is a policy dial, and the property it is chosen
/// for is this one: a corpus that is *substantially* unread behaves exactly as
/// it does today, and only a small hole buys any grading at all. Every older
/// coverage test in this crate is a two- or three-file fixture — 33% to 50%
/// blind — and keeps its original assertions because it sits past this line.
/// That is load-bearing rather than lucky, so it is measured here.
#[test]
fn the_ceiling_reaches_the_floor_well_before_the_corpus_is_half_unread() {
    let ceiling_at = |blind: usize, readable: usize| {
        let mut coverage = ExtractionCoverage::default();
        coverage.call_blind_files = blind;
        coverage.files_with_call_extraction = readable;
        coverage.cap(0.9)
    };

    // A quarter of the corpus unread is already all the way down.
    assert!(
        (ceiling_at(25, 75) - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "25% blind must be indistinguishable from today's behaviour: {}",
        ceiling_at(25, 75)
    );
    // Half, and a two-file fixture, likewise — this is the shape every older
    // test in this crate depends on.
    assert!((ceiling_at(50, 50) - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6);
    assert!((ceiling_at(1, 1) - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6);
    assert!((ceiling_at(1, 2) - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6);

    // And one file in a hundred is not the same claim as half the tree.
    assert!(
        ceiling_at(1, 99) > COVERAGE_LOSS_CONFIDENCE_CAP + 0.2,
        "a 1% hole must leave real headroom, or the grading buys nothing: {}",
        ceiling_at(1, 99)
    );

    // The crossover itself: somewhere between 5% and 20%, and stated so a
    // future exponent change has to come here and re-argue it rather than
    // silently moving where "degraded" starts to mean something else.
    let five_percent = ceiling_at(5, 95);
    let twenty_percent = ceiling_at(20, 80);
    assert!(
        five_percent > COVERAGE_LOSS_CONFIDENCE_CAP,
        "5% blind still grades: {five_percent}"
    );
    assert!(
        (twenty_percent - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "20% blind is on the floor: {twenty_percent}"
    );
}

/// A cluster is a strictly stronger claim, so it is priced strictly lower.
///
/// The single-symbol ceiling and the cluster ceiling must never coincide at the
/// same blind share, or `cap_cluster` is decorative — which is exactly what a
/// shared `cap()` made it.
#[test]
fn a_cluster_ceiling_is_stricter_than_a_single_symbol_ceiling() {
    let mut coverage = ExtractionCoverage::default();
    coverage.call_blind_files = 1;
    coverage.files_with_call_extraction = 199;

    let single = coverage.cap(0.9);
    for size in [2usize, 5, 20, 60, 100_000] {
        let cluster = coverage.cap_cluster(0.9, size);
        assert!(
            cluster <= single,
            "a whole-graph claim over {size} members must not outrank a \
             one-symbol claim: {cluster} vs {single}"
        );
        assert!(
            cluster.is_finite() && cluster >= COVERAGE_LOSS_CONFIDENCE_CAP,
            "and it must stay a number inside the band: {cluster}"
        );
    }

    // Bigger component, weaker claim — monotone in the size, which is the whole
    // reason the size is passed in.
    assert!(
        coverage.cap_cluster(0.9, 40) <= coverage.cap_cluster(0.9, 2),
        "a forty-symbol cluster is a weaker claim than a two-symbol one"
    );
}

/// A coverage record with no measured corpus behind it must fall to the floor
/// rather than divide by zero or read as fully covered.
///
/// Reachable: `ExtractionCoverage::default()` with `discovery_refused_files`
/// folded in afterwards is exactly the shape a build produces when discovery
/// refused every file it found.
#[test]
fn a_coverage_record_with_no_readable_files_falls_to_the_floor() {
    let mut coverage = ExtractionCoverage::default();
    coverage.discovery_refused_files = 3;
    coverage.files_with_call_extraction = 0;
    assert!(!coverage.is_complete());
    let got = coverage.cap(0.9);
    assert!(
        (got - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "nothing was read, so nothing is known: {got}"
    );
    assert!(got.is_finite(), "the ceiling must be a number: {got}");
}
