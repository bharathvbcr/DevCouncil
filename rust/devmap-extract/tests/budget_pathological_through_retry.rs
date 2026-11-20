//! The pathological C++ input (2,000 nested braces, ~4,000 bytes; the budget
//! doc records 131 s for it on `tree-sitter-cpp` 0.23), driven through the real
//! first-pass + serial-retry path (`extract_all_with_budgets`), next to a
//! corpus-sized legitimate file that merely runs slowly.
//!
//! `stress_hardening` feeds the same input to the parser with one direct
//! budget. That proves the parser honours a deadline; it does not prove the
//! *retry* is bounded, which is the other surface: a refusal in the parallel
//! pass is re-run under a larger budget, and a retry that did not honour its
//! own budget would turn the per-file bound back into the unbounded cost.
//!
//! "Slow" is injected through [`ParseBudgets`], the existing seam: a zero
//! first-pass budget refuses every file in the parallel pass exactly as a
//! contended machine does, without depending on the machine's load.
//!
//! Measured when this was written (debug build, this tree): the 2,000-brace
//! input no longer costs 131 s; it extracts in about a second, so it is
//! *recovered* by the retry rather than refused by it. The first test therefore
//! asserts the contract that holds either way (recovered whole, or refused with
//! a named budget reason, and in both cases inside a multiple of the retry
//! budget) rather than pinning the stale slowness. The second test forces the
//! refusal-at-retry state with a retry budget the input cannot meet, so the
//! "still refused" path is exercised on this exact input.

use devmap_extract::{extract_all_with_budgets, FileRef, ParseBudgets, BUDGET_REFUSAL_PREFIX};
use std::time::{Duration, Instant};

const RETRY: Duration = Duration::from_secs(4);

/// Corpus-sized: the largest real source the budget doc cites is 12,353 lines.
fn large_legitimate_python() -> String {
    let mut source = String::new();
    for i in 0..3_000 {
        source.push_str(&format!(
            "def fn_{i}(x):\n    y = helper_{i}(x)\n    return y + {i}\n\n"
        ));
    }
    source
}

fn hostile_cpp() -> String {
    format!("{}{}", "{".repeat(2_000), "}".repeat(2_000))
}

fn reason(extraction: &devmap_extract::Extraction) -> &str {
    match &extraction.parse_outcome {
        devmap_extract::ParseOutcome::Failed { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_slow_legitimate_file_is_recovered_and_the_pathological_one_is_bounded() {
    let legit = large_legitimate_python();
    let hostile = hostile_cpp();
    let files = [
        FileRef {
            path: "big.py",
            source: &legit,
        },
        FileRef {
            path: "hostile.cpp",
            source: &hostile,
        },
    ];
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: RETRY,
        retry_total: Duration::from_secs(120),
    };

    let started = Instant::now();
    let (out, report) = extract_all_with_budgets(&files, None, budgets, None);
    let elapsed = started.elapsed();

    // Both were refused in the parallel pass; that is the injected slowness.
    assert_eq!(report.first_pass_refused, 2, "{report:?}");
    assert_eq!(report.retried, 2, "{report:?}");

    // The legitimate file is recovered whole, not dropped.
    assert!(!out[0].is_parse_failure(), "{:?}", out[0].parse_outcome);
    assert_eq!(
        out[0]
            .symbols
            .iter()
            .filter(|s| s.name.starts_with("fn_"))
            .count(),
        3_000,
        "a recovered file must carry every declaration"
    );

    // The pathological file is either recovered or refused *by budget, named*.
    // It is never silently dropped and never left unexplained.
    if out[1].is_parse_failure() {
        assert!(out[1].is_budget_refusal(), "{:?}", out[1].parse_outcome);
        assert!(reason(&out[1]).starts_with(BUDGET_REFUSAL_PREFIX));
        assert_eq!(report.still_refused, vec!["hostile.cpp"], "{report:?}");
        assert_eq!(report.recovered, 1, "{report:?}");
    } else {
        assert_eq!(report.recovered, 2, "{report:?}");
        assert!(report.still_refused.is_empty(), "{report:?}");
    }

    // Bounded: two files, each held to the retry budget, with generous slack
    // because the stop is polled at walk boundaries.
    assert!(
        elapsed < RETRY * 20,
        "the retry did not honour its budget; took {elapsed:?}"
    );
}

#[test]
fn the_pathological_input_refused_at_the_retry_is_named_and_returns_promptly() {
    let hostile = hostile_cpp();
    let files = [FileRef {
        path: "hostile.cpp",
        source: &hostile,
    }];
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::ZERO,
        retry_total: Duration::from_secs(120),
    };

    let started = Instant::now();
    let (out, report) = extract_all_with_budgets(&files, None, budgets, None);
    let elapsed = started.elapsed();

    assert!(out[0].is_budget_refusal(), "{:?}", out[0].parse_outcome);
    let why = reason(&out[0]);
    assert!(why.starts_with(BUDGET_REFUSAL_PREFIX), "{why}");
    assert!(why.contains("refused again by the serial retry"), "{why}");
    assert_eq!(report.still_refused, vec!["hostile.cpp"], "{report:?}");
    assert_eq!(report.still_refused_total, 1);
    assert!(
        elapsed < Duration::from_secs(10),
        "a refused file must cost almost nothing; took {elapsed:?}"
    );
}
