//! The parse budget is wall clock, so a legitimate file can be refused in the
//! parallel pass purely because the machine is busy. These tests use a tiny
//! first-pass budget to force that refusal on a real file, and assert that the
//! serial retry recovers it, that a file the retry also cannot parse is named,
//! and that only budget refusals are retried.

use devmap_extract::{
    extract_all_with_budgets, extract_file_with_budget, retry_budget_refusals, FileRef,
    ParseBudgets, MAX_NAMED_BUDGET_REFUSALS,
};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const PY: &str = "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n";

const SMALL_FIRST: ParseBudgets = ParseBudgets {
    first_pass: Duration::ZERO,
    retry: Duration::from_secs(60),
};
const BOTH_TOO_SMALL: ParseBudgets = ParseBudgets {
    first_pass: Duration::ZERO,
    retry: Duration::ZERO,
};

fn file<'a>(path: &'a str, source: &'a str) -> FileRef<'a> {
    FileRef { path, source }
}

fn declares(extraction: &devmap_extract::Extraction, name: &str) -> bool {
    extraction.symbols.iter().any(|s| s.name == name)
}

#[test]
fn a_file_refused_in_the_parallel_pass_is_recovered_by_the_serial_retry() {
    let files = [file("a.py", PY), file("b.py", PY)];
    let (out, report) = extract_all_with_budgets(&files, None, SMALL_FIRST, None);

    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|e| !e.is_parse_failure()));
    assert!(out.iter().all(|e| declares(e, "alpha")));
    assert_eq!(report.first_pass_refused, 2);
    assert_eq!(report.retried, 2);
    assert_eq!(report.recovered, 2);
    assert_eq!(report.still_refused_total, 0);
    assert!(report.still_refused.is_empty());
    // Input order is preserved.
    assert_eq!(out[0].file_path, "a.py");
    assert_eq!(out[1].file_path, "b.py");
}

#[test]
fn the_recovered_extraction_equals_an_unhurried_one() {
    let files = [file("a.py", PY)];
    let (out, _) = extract_all_with_budgets(&files, None, SMALL_FIRST, None);
    let unhurried = extract_file_with_budget("a.py", PY, Duration::from_secs(60));
    assert_eq!(
        serde_json::to_string(&out[0]).unwrap(),
        serde_json::to_string(&unhurried).unwrap()
    );
}

#[test]
fn a_file_the_retry_cannot_parse_either_is_named_in_the_report() {
    let files = [file("a.py", PY), file("b.py", PY)];
    let (out, report) = extract_all_with_budgets(&files, None, BOTH_TOO_SMALL, None);

    assert!(out.iter().all(|e| e.is_budget_refusal()));
    assert_eq!(report.first_pass_refused, 2);
    assert_eq!(report.recovered, 0);
    assert_eq!(report.still_refused_total, 2);
    assert_eq!(report.still_refused, vec!["a.py", "b.py"]);
    assert!(report.names_all());
}

#[test]
fn the_named_list_is_bounded_and_carries_the_total() {
    let paths: Vec<String> = (0..MAX_NAMED_BUDGET_REFUSALS + 20)
        .map(|i| format!("f{i}.py"))
        .collect();
    let files: Vec<FileRef> = paths.iter().map(|p| file(p, PY)).collect();
    let (out, report) = extract_all_with_budgets(&files, None, BOTH_TOO_SMALL, None);

    assert!(out.iter().all(|e| e.is_budget_refusal()));
    assert_eq!(report.still_refused_total, files.len());
    assert_eq!(report.still_refused.len(), MAX_NAMED_BUDGET_REFUSALS);
    assert!(!report.names_all());
    // Files past the retry cap are refused, counted, and not retried.
    assert_eq!(
        report.retried,
        devmap_extract::treesitter::MAX_BUDGET_RETRIES
    );
    assert_eq!(
        report.not_retried,
        files.len() - devmap_extract::treesitter::MAX_BUDGET_RETRIES
    );
}

#[test]
fn zero_files_is_an_empty_report() {
    let (out, report) = extract_all_with_budgets(&[], None, SMALL_FIRST, None);
    assert!(out.is_empty());
    assert_eq!(report, Default::default());
}

#[test]
fn duplicate_paths_are_each_retried_and_reported() {
    let files = [file("a.py", PY), file("a.py", PY)];
    let (out, report) = extract_all_with_budgets(&files, None, BOTH_TOO_SMALL, None);
    assert!(out.iter().all(|e| e.is_budget_refusal()));
    assert_eq!(report.still_refused, vec!["a.py", "a.py"]);
    let (out, report) = extract_all_with_budgets(&files, None, SMALL_FIRST, None);
    assert!(out.iter().all(|e| declares(e, "alpha")));
    assert_eq!(report.recovered, 2);
}

#[test]
fn a_non_budget_failure_is_not_retried_or_relabelled() {
    // A malformed notebook fails at any budget that lets it start; the first
    // pass here is real-budget for it and refused-for-budget for the python.
    let files = [file("broken.ipynb", "not JSON"), file("a.py", PY)];
    let mut out = vec![
        extract_file_with_budget("broken.ipynb", "not JSON", Duration::from_secs(60)),
        extract_file_with_budget("a.py", PY, Duration::ZERO),
    ];
    let before = reason(&out[0]);
    let report = retry_budget_refusals(&files, &mut out, None, SMALL_FIRST, None);
    assert_eq!(reason_or_empty(&out[0]), before, "left exactly as reported");

    assert!(out[0].is_parse_failure());
    assert!(!out[0].is_budget_refusal());
    assert_eq!(report.first_pass_refused, 1, "only the python file");
    assert_eq!(report.retried, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.still_refused_total, 0);
    assert!(!out[1].is_parse_failure());
}

#[test]
fn cancellation_stops_the_retry_and_leaves_files_refused_and_counted() {
    let files = [file("a.py", PY), file("b.py", PY)];
    let cancel = AtomicBool::new(true);
    let (out, report) = extract_all_with_budgets(&files, None, SMALL_FIRST, Some(&cancel));

    assert!(out.iter().all(|e| e.is_budget_refusal()));
    assert_eq!(report.retried, 0);
    assert_eq!(report.not_retried, 2);
    assert_eq!(report.still_refused_total, 2);
    assert_eq!(report.still_refused, vec!["a.py", "b.py"]);
}

#[test]
fn retry_over_an_existing_result_set_leaves_clean_files_untouched() {
    let files = [file("a.py", PY), file("b.py", PY)];
    let mut out = vec![
        extract_file_with_budget("a.py", PY, Duration::from_secs(60)),
        extract_file_with_budget("b.py", PY, Duration::ZERO),
    ];
    let report = retry_budget_refusals(&files, &mut out, None, SMALL_FIRST, None);
    assert_eq!(report.first_pass_refused, 1);
    assert_eq!(report.recovered, 1);
    assert!(out.iter().all(|e| declares(e, "alpha")));
}

#[test]
fn progress_counts_each_file_once_and_failures_only_when_final() {
    let files = [file("a.py", PY), file("b.py", PY)];
    let recovered = devmap_extract::progress::FileProgress::default();
    extract_all_with_budgets(&files, Some(&recovered), SMALL_FIRST, None);
    let snap = recovered.snapshot();
    assert_eq!((snap.completed, snap.failed, snap.valid), (2, 0, true));

    let refused = devmap_extract::progress::FileProgress::default();
    extract_all_with_budgets(&files, Some(&refused), BOTH_TOO_SMALL, None);
    let snap = refused.snapshot();
    assert_eq!((snap.completed, snap.failed, snap.valid), (2, 2, true));
}

fn reason_or_empty(extraction: &devmap_extract::Extraction) -> String {
    match &extraction.parse_outcome {
        devmap_extract::ParseOutcome::Failed { reason } => reason.clone(),
        _ => String::new(),
    }
}

fn reason(extraction: &devmap_extract::Extraction) -> String {
    match &extraction.parse_outcome {
        devmap_extract::ParseOutcome::Failed { reason } => reason.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_file_refused_after_the_retry_says_so() {
    let files = [file("a.py", PY)];
    let (out, _) = extract_all_with_budgets(&files, None, BOTH_TOO_SMALL, None);
    let reason = reason(&out[0]);
    assert!(
        reason.starts_with(devmap_extract::BUDGET_REFUSAL_PREFIX),
        "{reason}"
    );
    assert!(
        reason.contains("refused again by the serial retry"),
        "{reason}"
    );
}

#[test]
fn a_file_past_the_retry_cap_says_it_was_not_retried_for_that_reason() {
    let paths: Vec<String> = (0..devmap_extract::treesitter::MAX_BUDGET_RETRIES + 1)
        .map(|i| format!("f{i}.py"))
        .collect();
    let files: Vec<FileRef> = paths.iter().map(|p| file(p, PY)).collect();
    let (out, _) = extract_all_with_budgets(&files, None, BOTH_TOO_SMALL, None);
    let last = reason(out.last().unwrap());
    assert!(
        last.contains("not retried") && last.contains("cap on serial retries"),
        "{last}"
    );
    assert!(reason(&out[0]).contains("refused again"));
}

#[test]
fn a_cancelled_retry_says_it_was_cancelled() {
    let files = [file("a.py", PY)];
    let cancel = AtomicBool::new(true);
    let (out, _) = extract_all_with_budgets(&files, None, SMALL_FIRST, Some(&cancel));
    let reason = reason(&out[0]);
    assert!(
        reason.contains("not retried") && reason.contains("cancelled"),
        "{reason}"
    );
    assert!(out[0].is_budget_refusal());
}

#[test]
fn the_build_progress_snapshot_reports_the_retry() {
    let files = [file("a.py", PY), file("b.py", PY)];
    let progress = devmap_extract::progress::FileProgress::default();
    extract_all_with_budgets(&files, Some(&progress), SMALL_FIRST, None);
    let counts = progress.snapshot().budget_retry;
    assert_eq!(
        (
            counts.refused,
            counts.retried,
            counts.recovered,
            counts.still_refused
        ),
        (2, 2, 2, 0)
    );
}

type FirstPass = (Vec<String>, Vec<devmap_extract::Extraction>);

fn first_pass_refused(count: usize) -> FirstPass {
    let paths: Vec<String> = (0..count).map(|i| format!("f{i}.py")).collect();
    let out = paths
        .iter()
        .map(|path| extract_file_with_budget(path, PY, Duration::ZERO))
        .collect();
    (paths, out)
}

#[test]
fn a_cancel_mid_retry_stops_at_the_next_file_and_reports_the_rest_as_cancelled() {
    use devmap_extract::retry_budget_refusals_observing;
    let (paths, mut out) = first_pass_refused(5);
    let files: Vec<FileRef<'_>> = paths.iter().map(|p| file(p, PY)).collect();
    let cancel = AtomicBool::new(false);
    let mut retried_calls = 0;
    let started = std::time::Instant::now();
    let report = retry_budget_refusals_observing(
        &files,
        &mut out,
        None,
        SMALL_FIRST,
        Some(&cancel),
        &mut |_| {
            retried_calls += 1;
            if retried_calls == 2 {
                // Twice on purpose: a repeated cancel is harmless.
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        },
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(retried_calls, 2, "no file is re-run after the cancel");
    assert_eq!(report.first_pass_refused, 5);
    assert_eq!(report.retried, 2);
    assert_eq!(report.recovered, 2);
    assert_eq!(report.cancelled, 3, "{report:?}");
    assert_eq!(report.not_retried, 3);
    assert!(report.was_cancelled());
    assert_eq!(report.still_refused, vec!["f2.py", "f3.py", "f4.py"]);
    for cancelled in &out[2..] {
        let reason = reason(cancelled);
        assert!(reason.contains("cancelled"), "{reason}");
    }
    assert!(!out[0].is_budget_refusal() && !out[1].is_budget_refusal());
}

#[test]
fn a_cancel_before_the_first_retried_file_cancels_all_of_them() {
    let (paths, mut out) = first_pass_refused(3);
    let files: Vec<FileRef<'_>> = paths.iter().map(|p| file(p, PY)).collect();
    let cancel = AtomicBool::new(true);
    let report = retry_budget_refusals(&files, &mut out, None, SMALL_FIRST, Some(&cancel));
    assert_eq!((report.retried, report.cancelled), (0, 3), "{report:?}");
}

#[test]
fn a_cancel_after_the_last_retried_file_is_not_a_cancellation() {
    use devmap_extract::retry_budget_refusals_observing;
    let (paths, mut out) = first_pass_refused(3);
    let files: Vec<FileRef<'_>> = paths.iter().map(|p| file(p, PY)).collect();
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    let report = retry_budget_refusals_observing(
        &files,
        &mut out,
        None,
        SMALL_FIRST,
        Some(&cancel),
        &mut |_| {
            calls += 1;
            if calls == 3 {
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        },
    );
    assert!(!report.was_cancelled(), "{report:?}");
    assert_eq!((report.retried, report.recovered), (3, 3));
    assert!(out.iter().all(|e| !e.is_parse_failure()));
}

#[test]
fn the_retry_cap_is_not_counted_as_a_cancellation() {
    let count = devmap_extract::treesitter::MAX_BUDGET_RETRIES + 2;
    let (paths, mut out) = first_pass_refused(count);
    let files: Vec<FileRef<'_>> = paths.iter().map(|p| file(p, PY)).collect();
    let report = retry_budget_refusals(&files, &mut out, None, BOTH_TOO_SMALL, None);
    assert_eq!(report.not_retried, 2, "{report:?}");
    assert_eq!(report.cancelled, 0);
    assert!(!report.was_cancelled());
}
