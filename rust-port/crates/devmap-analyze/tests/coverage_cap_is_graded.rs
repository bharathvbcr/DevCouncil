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
    let outcome =
        analyze_liveness_with_coverage(extractions, &resolution, DiscoveryCoverage::none());
    (outcome.reports, outcome.coverage)
}

/// A coverage record with a chosen blind share and nothing else, built in one
/// expression so the ratio under test is readable at every call site.
fn blindness(blind: usize, readable: usize) -> ExtractionCoverage {
    ExtractionCoverage {
        call_blind_files: blind,
        files_with_call_extraction: readable,
        ..ExtractionCoverage::default()
    }
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
        let coverage = blindness(blind, 100);
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
        let coverage = blindness(blind, 100);
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
    let ceiling_at = |blind: usize, readable: usize| blindness(blind, readable).cap(0.9);

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

/// A cluster is a stronger claim, so it is priced lower — **where the clamp
/// does not mask the difference.**
///
/// The doc that stood here said the two ceilings "must never coincide at the
/// same blind share, or `cap_cluster` is decorative", and the assertion below it
/// was `cluster <= single`, which is satisfied by equality. They did coincide,
/// at the very blind share the test chose: `blindness(1, 199)` is 0.5% blind, so
/// both `(1-s)^8` and `(1-s)^10` exceed `HIGHEST_DEGRADED_CONFIDENCE` and both
/// clamp to it. The test passed on `<=` while the property it stated was false.
///
/// The true property is narrower and is what is asserted now. Both ceilings are
/// `clamp(·, FLOOR, HIGHEST)`, so they necessarily agree at both ends:
///
/// * **both at `HIGHEST`** while even the compounded exponent stays above it —
///   a corpus barely blind at all, where the size of the component genuinely is
///   not the binding constraint;
/// * **both at `FLOOR`** once even the un-compounded exponent has fallen below
///   it — a corpus so blind that no claim about it survives.
///
/// Between those, they must separate *strictly*, or the compounding is
/// decorative for real. That band is what is swept here.
#[test]
fn a_cluster_ceiling_is_stricter_than_a_single_symbol_ceiling() {
    // Never outranks, at any share and any size. The weak half of the contract,
    // kept because it is the half a future formula is most likely to break.
    for (blind, readable) in [
        (1usize, 199usize),
        (1, 99),
        (2, 98),
        (5, 95),
        (11, 89),
        (50, 50),
    ] {
        let coverage = blindness(blind, readable);
        let single = coverage.cap(0.9);
        for size in [2usize, 5, 20, 60, 100_000] {
            let cluster = coverage.cap_cluster(0.9, size);
            assert!(
                cluster <= single,
                "a whole-graph claim over {size} members must not outrank a \
                 one-symbol claim (blind={blind}/{readable}): {cluster} vs {single}"
            );
            assert!(
                cluster.is_finite() && cluster >= COVERAGE_LOSS_CONFIDENCE_CAP,
                "and it must stay a number inside the band: {cluster}"
            );
        }
    }

    // The strong half: inside the band, the compounding really separates them.
    // At 5% blind the single-symbol ceiling is 0.663 and a five-member component
    // is 0.513 — a whole tier of difference, not a rounding step.
    let banded = blindness(5, 95);
    let single = banded.cap(0.9);
    assert!(
        single < HIGHEST_DEGRADED_CONFIDENCE && single > COVERAGE_LOSS_CONFIDENCE_CAP,
        "fixture assumption: this share must sit inside the clamp, or the \
         separation below is masked: {single}"
    );
    for size in [2usize, 5, 20] {
        let cluster = banded.cap_cluster(0.9, size);
        assert!(
            cluster < single - 1e-6,
            "inside the band the compounding must separate the two strictly, \
             not merely fail to invert them: {cluster} vs {single} at size {size}"
        );
    }

    // Bigger component, weaker claim — monotone in the size, which is the whole
    // reason the size is passed in.
    assert!(
        banded.cap_cluster(0.9, 40) < banded.cap_cluster(0.9, 2),
        "a forty-symbol cluster is a weaker claim than a two-symbol one"
    );
}

/// **M2.** The one blind share at which the ladder still flattens, stated.
///
/// `cap` is a ceiling and its combinator is `min`, so wherever the ceiling falls
/// below a finding's own confidence, every finding above it collapses onto the
/// ceiling. For the two tiers that matter — 0.9 ("no edge names this") and 0.4
/// ("something names it and we could not say what") — that is the window where
/// the ceiling sits in `[0.35, 0.4]`: from `1 - 0.4^(1/8) = 10.82%` blind to the
/// floor crossover at `1 - 0.35^(1/8) = 12.3%`.
///
/// It is not removable without abandoning `min` for a multiplicative degrade,
/// which would lower *every* finding — including a 0.4 one in a barely-degraded
/// corpus — and cross tier boundaries for findings the coverage hole says
/// nothing about. So it is bounded and pinned instead: a window 1.5 percentage
/// points wide, inside which both claims land in `ambiguous`, which is the tier
/// that means "do not act" for both of them.
///
/// `the_cap_is_monotone_at_every_blind_share` checks non-decreasing and cannot
/// see this; a window that widened to ten points would still pass it.
#[test]
fn the_ladder_flattens_only_inside_a_narrow_stated_window() {
    let separated = |blind: usize, readable: usize| {
        let coverage = blindness(blind, readable);
        coverage.cap(0.9) > coverage.cap(0.4) + 1e-6
    };

    // Below the window: the two tiers are distinct.
    assert!(separated(10, 90), "10% blind must still rank the two apart");
    // Inside it: they are not, and that is the stated cost.
    assert!(
        !separated(115, 885),
        "11.5% blind is inside the window this test exists to bound"
    );
    // Above it: both are on the floor, which is the pre-grading behaviour the
    // floor was kept to preserve.
    assert!(!separated(20, 80), "20% blind is on the floor for both");

    // And the window is narrow. Swept at tenth-of-a-percent resolution so a
    // future exponent or floor that widened it has to come here and say so.
    // `blind` counts twentieths of a percent of a 2,000-file corpus, so the
    // sweep runs from 0.05% to 20% at 0.05% resolution.
    let flat: Vec<usize> = (1..=400)
        .filter(|blind| !separated(*blind, 2000 - *blind))
        .collect();
    let first = *flat
        .first()
        .expect("the window is non-empty by construction");
    assert!(
        (205..=225).contains(&first),
        "the flattening must begin at ~10.8% blind, not earlier: it begins at \
         {:.2}%",
        first as f32 / 20.0
    );
    // And once it starts it does not stop: everything above the window is on
    // the floor for both tiers, so there is exactly one flat region, not a
    // scatter of them.
    assert_eq!(
        flat.len(),
        400 - first + 1,
        "the flat region must be contiguous from {:.2}% upward, not a scatter",
        first as f32 / 20.0
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
    let coverage = ExtractionCoverage {
        discovery_refused_files: 3,
        files_with_call_extraction: 0,
        ..ExtractionCoverage::default()
    };
    assert!(!coverage.is_complete());
    let got = coverage.cap(0.9);
    assert!(
        (got - COVERAGE_LOSS_CONFIDENCE_CAP).abs() < 1e-6,
        "nothing was read, so nothing is known: {got}"
    );
    assert!(got.is_finite(), "the ceiling must be a number: {got}");
}

/// **M9.** The two boundary inputs the cluster ceiling accepts, priced.
///
/// Neither is reachable from the producer — Tarjan emits no empty component,
/// and the node cap refuses long before a component of `usize::MAX` — so both
/// are clamps over inputs the type allows and the caller cannot supply. Pinned
/// anyway, because a clamp whose behaviour nobody has stated is a clamp a later
/// edit will "simplify".
#[test]
fn the_cluster_ceiling_is_total_over_the_sizes_its_type_permits() {
    let coverage = blindness(5, 95);

    // Zero members is priced as one: `(1 - s)^8`, the single-symbol ceiling. A
    // claim about nothing must not come back stronger than a claim about
    // something, and `powi(8 + 0)` would be exactly that.
    assert!(
        (coverage.cap_cluster(0.9, 0) - coverage.cap_cluster(0.9, 1)).abs() < 1e-6,
        "a zero-member component is priced as a one-member one: {} vs {}",
        coverage.cap_cluster(0.9, 0),
        coverage.cap_cluster(0.9, 1)
    );
    assert!(
        coverage.cap_cluster(0.9, 0) <= coverage.cap(0.9),
        "and never above the single-symbol ceiling"
    );

    // And the upper end saturates rather than overflowing `powi`'s exponent.
    let huge = coverage.cap_cluster(0.9, usize::MAX);
    assert!(
        huge.is_finite()
            && (COVERAGE_LOSS_CONFIDENCE_CAP..=HIGHEST_DEGRADED_CONFIDENCE).contains(&huge),
        "an absurd membership must stay inside the band: {huge}"
    );
    assert!(
        (huge - coverage.cap_cluster(0.9, 64)).abs() < 1e-6,
        "past `CLUSTER_COMPOUNDING_MEMBER_CAP` the size stops being informative \
         and the ceiling stops moving: {huge}"
    );
}
