//! Adversarial probes for the build-chain cancellation (ft-fb394194).

use devmap_extract::{extract_file_with_budget, retry_budget_refusals_observing, FileRef, ParseBudgets};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PY: &str = "def alpha():\n    return 1\n";
const BOTH_ZERO: ParseBudgets = ParseBudgets {
    first_pass: Duration::ZERO,
    retry: Duration::ZERO,
    retry_total: Duration::from_secs(600),
};

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn refused(paths: &[String]) -> Vec<devmap_extract::Extraction> {
    paths
        .iter()
        .map(|p| extract_file_with_budget(p, PY, Duration::ZERO))
        .collect()
}

fn refs(paths: &[String]) -> Vec<FileRef<'_>> {
    paths
        .iter()
        .map(|p| FileRef {
            path: p,
            source: PY,
        })
        .collect()
}

/// Invariants the report must hold for every (n, trip point) pair.
#[test]
fn cancelled_is_a_subset_of_not_retried_for_every_trip_point() {
    let mut rng = 0x9E3779B97F4A7C15u64;
    for _ in 0..400 {
        let n = (xorshift(&mut rng) % 45) as usize;
        let trip = (xorshift(&mut rng) % (n as u64 + 3)) as usize;
        let paths: Vec<String> = (0..n).map(|i| format!("f{i}.py")).collect();
        let files = refs(&paths);
        let mut out = refused(&paths);
        let cancel = AtomicBool::new(false);
        let mut seen = 0usize;
        let report = retry_budget_refusals_observing(
            &files,
            &mut out,
            None,
            BOTH_ZERO,
            Some(&cancel),
            &mut |_| {
                seen += 1;
                if seen == trip {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
        );
        assert!(report.cancelled <= report.not_retried, "n={n} trip={trip}");
        assert_eq!(
            report.retried + report.not_retried,
            report.first_pass_refused,
            "n={n} trip={trip}"
        );
        assert!(report.retried <= n);
    }
}

/// A cancel that lands after the retry phase's deadline stops nothing: the
/// remaining refusals are deadline-refused whether or not the flag is set.
/// Counting them as `cancelled` discards a finished build (the store-level
/// contract says "a cancel after the last file is a complete build").
///
/// The deadline is passed deterministically: the first file's callback sleeps
/// past it and then trips the flag, so every later file sees both.
#[test]
fn a_cancel_after_the_retry_deadline_does_not_cancel_deadline_refused_files() {
    let n = 6;
    let budgets = ParseBudgets {
        retry_total: Duration::from_millis(500),
        ..BOTH_ZERO
    };
    let paths: Vec<String> = (0..n).map(|i| format!("f{i}.py")).collect();
    let files = refs(&paths);
    let mut out = refused(&paths);
    let cancel = AtomicBool::new(false);
    let report = retry_budget_refusals_observing(
        &files,
        &mut out,
        None,
        budgets,
        Some(&cancel),
        &mut |_| {
            std::thread::sleep(Duration::from_millis(700));
            cancel.store(true, Ordering::SeqCst);
        },
    );
    assert_eq!(report.retried, 1, "{report:?}");
    assert_eq!(
        report.cancelled, 0,
        "the deadline passed before the cancel; the {} remaining refusals were never going to \
         be retried, yet the report calls them cancelled: {report:?}",
        n - 1
    );
    assert_eq!(report.not_retried, n - 1);
    assert!(!report.was_cancelled());
}

/// A flag set before the call does no work at all.
#[test]
fn a_pre_tripped_flag_retries_nothing() {
    let paths: Vec<String> = (0..5).map(|i| format!("f{i}.py")).collect();
    let files = refs(&paths);
    let mut out = refused(&paths);
    let cancel = AtomicBool::new(true);
    let mut calls = 0;
    let report = retry_budget_refusals_observing(
        &files,
        &mut out,
        None,
        ParseBudgets::default(),
        Some(&cancel),
        &mut |_| calls += 1,
    );
    assert_eq!((calls, report.retried, report.cancelled), (0, 0, 5));
    assert!(out.iter().all(|e| e.is_budget_refusal()));
}
