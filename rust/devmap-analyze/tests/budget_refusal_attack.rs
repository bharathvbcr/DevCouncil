//! Adversarial probes for the budget-refused clause of `degraded_reason`.
//! Each test asserts the behaviour the lane intends; a failure is a break.

use devmap_analyze::{extraction_coverage, ExtractionCoverage};
use devmap_extract::model::{Extraction, ParseOutcome, BUDGET_REFUSAL_PREFIX};
use devmap_extract::{extract_file, extract_file_with_budget};
use std::time::Duration;

fn refused(path: &str, source: &str) -> Extraction {
    let ext = extract_file_with_budget(path, source, Duration::ZERO);
    assert!(
        ext.is_budget_refusal(),
        "{path} must be a refusal: {:?}",
        ext.parse_outcome
    );
    ext
}

fn failed_with(path: &str, source: &str, reason: String) -> Extraction {
    let mut ext = extract_file(path, source);
    ext.parse_outcome = ParseOutcome::Failed { reason };
    assert!(ext.is_parse_failure(), "{path} must be a parse failure");
    ext
}

fn reason_of(cov: &ExtractionCoverage) -> String {
    cov.degraded_reason().expect("degraded")
}

const GO: &str = "package p\n\nfunc a() int { return 1 }\n";
const PY: &str = "def a():\n    return 1\n";
const RS: &str = "fn a() -> i32 { 1 }\n";
const TS: &str = "export function a() { return 1 }\n";

#[test]
fn p1_count_never_exceeds_parse_failed_across_languages() {
    let exts = vec![
        refused("a.go", GO),
        refused("b.py", PY),
        refused("c.rs", RS),
        refused("d.ts", TS),
        extract_file("ok.go", GO),
    ];
    let cov = extraction_coverage(&exts);
    assert!(
        cov.budget_refused_files <= cov.parse_failed_files,
        "{cov:?}"
    );
    assert_eq!(cov.budget_refused_files, 4, "{cov:?}");
    assert_eq!(cov.parse_failed_files, 4, "{cov:?}");
}

#[test]
fn p1_prefix_with_suffix_text_counts_once_each() {
    let exts = vec![
        failed_with(
            "a.go",
            GO,
            format!("{BUDGET_REFUSAL_PREFIX}took 9s of 5s, retry 30s"),
        ),
        failed_with("b.py", PY, BUDGET_REFUSAL_PREFIX.to_string()),
    ];
    let cov = extraction_coverage(&exts);
    assert_eq!(cov.budget_refused_files, 2, "{cov:?}");
}

#[test]
fn p1_prefix_mid_string_is_a_grammar_failure_not_a_refusal() {
    let exts = vec![
        failed_with(
            "a.go",
            GO,
            format!("grammar fault; {BUDGET_REFUSAL_PREFIX}over 5s"),
        ),
        failed_with("b.py", PY, format!(" {BUDGET_REFUSAL_PREFIX}over 5s")),
        failed_with("c.rs", RS, BUDGET_REFUSAL_PREFIX.to_uppercase()),
    ];
    let cov = extraction_coverage(&exts);
    assert_eq!(cov.parse_failed_files, 3);
    assert_eq!(cov.budget_refused_files, 0, "{cov:?}");
    assert!(
        !reason_of(&cov).contains("budget-refused"),
        "{}",
        reason_of(&cov)
    );
}

#[test]
fn p1_refusal_prefixed_reason_on_a_prose_file_is_not_counted() {
    // A `.md` is NotApplicable: never a parse failure, so never a refusal.
    let mut md = extract_file("README.md", "# hi\n");
    md.parse_outcome = ParseOutcome::Failed {
        reason: format!("{BUDGET_REFUSAL_PREFIX}x"),
    };
    let cov = extraction_coverage(&[md, extract_file("ok.go", GO)]);
    assert_eq!(cov.parse_failed_files, 0);
    assert_eq!(cov.budget_refused_files, 0, "{cov:?}");
}

#[test]
fn p1_pattern_recovered_file_is_never_budget_counted() {
    let mut fb = extract_file("x.go", GO);
    fb.parse_outcome = ParseOutcome::Fallback {
        reason: format!("{BUDGET_REFUSAL_PREFIX}looks like one"),
    };
    let cov = extraction_coverage(&[fb, refused("y.py", PY)]);
    assert_eq!(cov.pattern_recovered_files, 1, "{cov:?}");
    assert_eq!(cov.parse_failed_files, 1, "{cov:?}");
    assert_eq!(cov.budget_refused_files, 1, "{cov:?}");
}

#[test]
fn p1_duplicate_path_does_not_inflate_the_count() {
    // The same path twice, one refused and one grammar-failed: the refused set
    // is keyed by path, so the grammar failure must not ride along.
    let exts = vec![
        refused("dup.go", GO),
        failed_with("dup.go", GO, "grammar fault".to_string()),
    ];
    let cov = extraction_coverage(&exts);
    assert_eq!(cov.parse_failed_files, 2);
    assert_eq!(cov.budget_refused_files, 1, "{cov:?}");
}

#[test]
fn p2_all_refused_reads_one_of_one() {
    let cov = extraction_coverage(&[refused("a.go", GO)]);
    let r = reason_of(&cov);
    assert!(r.contains("1 file(s) failed to parse"), "{r}");
    assert!(
        r.contains("1 of the parse failure(s) are budget-refused"),
        "{r}"
    );
}

#[test]
fn p2_zero_refused_has_no_budget_clause() {
    let cov = extraction_coverage(&[failed_with("a.go", GO, "grammar fault".into())]);
    let r = reason_of(&cov);
    assert!(!r.contains("budget"), "{r}");
    assert!(!r.contains("wall-clock"), "{r}");
    assert!(!r.contains("re-extracts"), "{r}");
}

#[test]
fn p2_ten_thousand_refused_stay_bounded() {
    let one = reason_of(&extraction_coverage(&[refused("a.go", GO)]));
    let exts: Vec<Extraction> = (0..10_000)
        .map(|i| failed_with(&format!("f{i}.go"), GO, format!("{BUDGET_REFUSAL_PREFIX}x")))
        .collect();
    let cov = extraction_coverage(&exts);
    let r = reason_of(&cov);
    assert_eq!(cov.budget_refused_files, 10_000);
    assert!(r.contains("10000 of the parse failure(s)"), "{r}");
    assert!(
        r.len() < one.len() + 64,
        "unbounded growth: {} vs {}",
        r.len(),
        one.len()
    );
}

#[test]
fn p2_no_doubled_capped_phrasing_and_no_contradiction() {
    let cov = extraction_coverage(&[refused("a.go", GO), extract_file("b.go", GO)]);
    let r = reason_of(&cov);
    assert_eq!(
        r.matches("capped").count(),
        r.matches("capped below the confident tier").count(),
        "{r}"
    );
    assert_eq!(r.matches("confident tier").count(), 1, "{r}");
    let clause = &r[r.find("; 1 of the parse failure").expect("clause")..];
    assert!(!clause.contains("confident"), "{clause}");
    assert!(!clause.contains("permanent"), "{clause}");
    assert_eq!(r.matches("budget-refused").count(), 1, "{r}");
}

#[test]
fn p2_clause_order_is_call_blind_before_budget_before_partial() {
    let mut cov = extraction_coverage(&[refused("a.go", GO)]);
    cov.call_blind_files = 2;
    cov.partial_parse_files = 3;
    let r = reason_of(&cov);
    let (c, b, p) = (
        r.find("no call extractor").unwrap(),
        r.find("budget-refused").unwrap(),
        r.find("ERROR/MISSING").unwrap(),
    );
    assert!(c < b && b < p, "{r}");
}

#[test]
fn p2_hand_set_count_above_parse_failed_is_not_printed() {
    // Aggregate-only records set counters by hand: "5 of 1" is impossible and
    // nothing in the type stops it printing.
    let cov = ExtractionCoverage {
        parse_failed_files: 1,
        budget_refused_files: 5,
        ..Default::default()
    };
    let r = reason_of(&cov);
    assert!(
        !r.contains("5 of the parse failure"),
        "impossible ratio printed: {r}"
    );
}
