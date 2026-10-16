//! The call graph built on a busy machine must equal the one built on an idle
//! one, or the difference must be surfaced as degraded -- never silently absent.
//!
//! Edges are resolved from what extraction yields (symbols, imports, calls,
//! references), and extraction is bounded by a wall-clock budget. This target
//! cannot link the resolver, so it pins the contract at the seam the edge set
//! is built from: the *edge inputs* -- every file's symbols, imports, calls
//! and references -- under contention equal the idle ones, and any file that
//! differs is a budget refusal the report names and the extraction itself
//! says so.
//!
//! Two ways of making contention, because only one is deterministic:
//! * a first-pass budget of zero refuses every file in the parallel pass for
//!   certain (the load a loaded CI box produces at random), and the serial
//!   retry must restore the idle result byte for byte;
//! * real busy threads at twice the core count while the production budgets
//!   run, where the outcome is not predictable but the invariant still is.
//!
//! Bounded: a few dozen small files and a hard wall-clock cap on the busy
//! threads, well under the 60 s the suite allows a test.

use devmap_extract::{
    extract_all_with_budgets, BudgetRetryReport, Extraction, FileRef, ParseBudgets,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FILES: usize = 24;

fn source(i: usize) -> String {
    format!(
        "import helper_{i}\n\n\
         def entry_{i}():\n    return mid_{i}() + helper_{i}.leaf()\n\n\
         def mid_{i}():\n    return leaf_{i}()\n\n\
         def leaf_{i}():\n    return {i}\n"
    )
}

fn corpus() -> Vec<(String, String)> {
    (0..FILES)
        .map(|i| (format!("pkg/m{i:02}.py"), source(i)))
        .collect()
}

fn refs(corpus: &[(String, String)]) -> Vec<FileRef<'_>> {
    corpus
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect()
}

/// Everything an edge is resolved from, and nothing the clock can touch.
fn edge_inputs(extraction: &Extraction) -> String {
    serde_json::to_string(&(
        &extraction.file_path,
        &extraction.symbols,
        &extraction.imports,
        &extraction.calls,
        &extraction.references,
    ))
    .unwrap()
}

fn idle(corpus: &[(String, String)]) -> Vec<String> {
    let budgets = ParseBudgets {
        first_pass: Duration::from_secs(60),
        retry: Duration::from_secs(60),
    };
    let (out, report) = extract_all_with_budgets(&refs(corpus), None, budgets, None);
    assert_eq!(
        report.first_pass_refused, 0,
        "the idle baseline was refused"
    );
    assert!(out.iter().all(|e| !e.is_parse_failure()));
    assert!(
        out.iter().all(|e| !e.calls.is_empty()),
        "the baseline must carry edge inputs or equality is vacuous"
    );
    out.iter().map(edge_inputs).collect()
}

#[test]
fn a_universally_refused_first_pass_is_restored_by_the_retry_to_the_idle_edge_inputs() {
    // At most the retry cap, because the retry re-runs only that many files
    // (see the next test for what happens past it).
    let corpus: Vec<_> = corpus()
        .into_iter()
        .take(devmap_extract::treesitter::MAX_BUDGET_RETRIES)
        .collect();
    let baseline = idle(&corpus);
    let contended = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::from_secs(60),
    };
    let (out, report) = extract_all_with_budgets(&refs(&corpus), None, contended, None);

    assert!(report.first_pass_refused > 0, "contention was not forced");
    assert_eq!(report.recovered, report.first_pass_refused);
    assert_eq!(report.still_refused_total, 0);
    let got: Vec<String> = out.iter().map(edge_inputs).collect();
    assert_eq!(got, baseline, "edge inputs drifted under contention");
}

#[test]
fn past_the_retry_cap_the_unrecovered_files_are_named_not_silently_absent() {
    let cap = devmap_extract::treesitter::MAX_BUDGET_RETRIES;
    let corpus: Vec<_> = (0..cap + 8)
        .map(|i| (format!("pkg/m{i:02}.py"), source(i)))
        .collect();
    let baseline = idle(&corpus);
    let contended = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::from_secs(60),
    };
    let (out, report) = extract_all_with_budgets(&refs(&corpus), None, contended, None);

    // Every file the first pass refused beyond the cap keeps its refusal, so
    // its edges are missing from the idle comparison -- and must say so.
    assert!(report.retried <= cap);
    assert_eq!(
        report.still_refused_total,
        report.first_pass_refused - report.recovered
    );
    assert_degraded_or_equal(&corpus, &baseline, &out, &report);
}

#[test]
fn when_the_retry_is_refused_too_every_difference_is_named_as_degraded() {
    let corpus = corpus();
    let baseline = idle(&corpus);
    let starved = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::ZERO,
    };
    let (out, report) = extract_all_with_budgets(&refs(&corpus), None, starved, None);
    assert_degraded_or_equal(&corpus, &baseline, &out, &report);
    // Not merely tolerated: with nothing parsed, every file differs, so every
    // file must be named.
    assert_eq!(report.still_refused_total, FILES);
    assert_eq!(
        report.still_refused.len(),
        FILES.min(devmap_extract::MAX_NAMED_BUDGET_REFUSALS)
    );
    assert!(out.iter().all(|e| e.is_budget_refusal()));
}

/// The invariant itself: a file whose edge inputs differ from the idle ones is a
/// budget refusal, and the report's total counts it.
fn assert_degraded_or_equal(
    corpus: &[(String, String)],
    baseline: &[String],
    out: &[Extraction],
    report: &BudgetRetryReport,
) {
    assert_eq!(out.len(), corpus.len());
    let mut differing = 0;
    for (extraction, expected) in out.iter().zip(baseline) {
        if edge_inputs(extraction) != *expected {
            differing += 1;
            assert!(
                extraction.is_budget_refusal(),
                "{} differs from the idle edge inputs without saying why: {:?}",
                extraction.file_path,
                extraction.parse_outcome
            );
        }
    }
    assert_eq!(
        report.still_refused_total, differing,
        "the report must count exactly the files whose edges are missing"
    );
}

#[test]
fn real_cpu_contention_never_changes_edge_inputs_silently() {
    let corpus = corpus();
    let baseline = idle(&corpus);

    let stop = AtomicBool::new(false);
    let burners = std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8);
    // Production budgets, the way a build runs them.
    let budgets = ParseBudgets::default();
    let (out, report) = std::thread::scope(|scope| {
        for _ in 0..burners {
            scope.spawn(|| {
                let started = std::time::Instant::now();
                let mut x = 1u64;
                // Hard cap so a failure of the main thread cannot leave the
                // machine spinning.
                while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(30) {
                    for _ in 0..10_000 {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    }
                    std::hint::black_box(x);
                }
            });
        }
        let result = extract_all_with_budgets(&refs(&corpus), None, budgets, None);
        stop.store(true, Ordering::Relaxed);
        result
    });

    assert_degraded_or_equal(&corpus, &baseline, &out, &report);
}
