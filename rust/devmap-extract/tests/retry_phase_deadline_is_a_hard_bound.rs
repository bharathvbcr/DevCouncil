//! `RETRY_PHASE_DEADLINE` is the longest a retry phase may hold the store's
//! writer lock, so it has to bound the phase itself and not only the gaps
//! between files. It was checked only before each file: a file that passed the
//! check ran its whole `budgets.retry` (25 s by default) past the deadline.

use devmap_extract::{extract_file_with_budget, retry_budget_refusals, FileRef, ParseBudgets};
use std::time::{Duration, Instant};

/// A well-formed Rust file big enough that parsing it takes at least
/// `at_least`, and how long it took (best of two runs, so a cold cache does
/// not inflate the figure the assertions are scaled by). Calibrated rather than
/// fixed so the test means the same thing in a debug and a release build.
fn slow_source(at_least: Duration) -> (String, Duration) {
    let mut functions = 2_000usize;
    loop {
        let mut source = String::new();
        for i in 0..functions {
            source.push_str(&format!("fn f{i}(a: i32) -> i32 {{ a + {i} }}\n"));
        }
        let timed = || {
            let started = Instant::now();
            let extraction = extract_file_with_budget("big.rs", &source, Duration::from_secs(120));
            assert!(
                !extraction.is_parse_failure(),
                "calibration run was refused"
            );
            started.elapsed()
        };
        let took = timed().min(timed());
        if took >= at_least || functions > 2_000_000 {
            return (source, took);
        }
        functions *= 2;
    }
}

fn first_pass_refused<'a>(
    path: &'a str,
    source: &'a str,
) -> (FileRef<'a>, devmap_extract::Extraction) {
    (
        FileRef { path, source },
        extract_file_with_budget(path, source, Duration::ZERO),
    )
}

#[test]
fn a_file_that_starts_before_the_deadline_cannot_run_past_it() {
    let (source, took) = slow_source(Duration::from_millis(800));
    let (file, refused) = first_pass_refused("big.rs", &source);
    assert!(refused.is_budget_refusal());
    let files = [file];
    let mut extractions = [refused];

    // The phase may run a quarter of what the file needs; the per-file retry
    // budget is far larger, so only the phase deadline can stop it early.
    let phase = took / 4;
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::from_secs(120),
        retry_total: phase,
    };
    let started = Instant::now();
    let report = retry_budget_refusals(&files, &mut extractions, None, budgets, None);
    let elapsed = started.elapsed();

    assert!(
        elapsed < took / 2,
        "the {phase:?} phase deadline let one file run {elapsed:?} (it needs {took:?} to finish)"
    );
    // Cut short is refused, never parsed, and it is accounted as a file that
    // was re-run and did not recover.
    assert!(
        extractions[0].is_budget_refusal(),
        "{:?}",
        extractions[0].parse_outcome
    );
    assert_eq!(
        (
            report.retried,
            report.recovered,
            report.not_retried,
            report.still_refused_total
        ),
        (1, 0, 0, 1),
        "{report:?}"
    );
    assert_eq!(report.still_refused, vec!["big.rs"]);
}

#[test]
fn a_file_cut_short_by_the_deadline_says_so() {
    let (source, took) = slow_source(Duration::from_millis(800));
    let (file, refused) = first_pass_refused("big.rs", &source);
    let files = [file];
    let mut extractions = [refused];
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::from_secs(120),
        retry_total: took / 4,
    };
    retry_budget_refusals(&files, &mut extractions, None, budgets, None);
    let devmap_extract::model::ParseOutcome::Failed { reason } = &extractions[0].parse_outcome
    else {
        panic!("expected a refusal: {:?}", extractions[0].parse_outcome);
    };
    assert!(
        reason.contains("phase deadline"),
        "a reader of parse_failed must be able to tell a deadline cut from a spent retry budget: {reason}"
    );
}

#[test]
fn a_phase_with_room_still_gives_a_file_its_whole_retry_budget() {
    let (source, took) = slow_source(Duration::from_millis(300));
    let (file, refused) = first_pass_refused("big.rs", &source);
    let files = [file];
    let mut extractions = [refused];
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: took * 20,
        retry_total: took * 40,
    };
    let report = retry_budget_refusals(&files, &mut extractions, None, budgets, None);
    assert!(!extractions[0].is_parse_failure());
    assert_eq!(
        (report.retried, report.recovered, report.still_refused_total),
        (1, 1, 0)
    );
}
