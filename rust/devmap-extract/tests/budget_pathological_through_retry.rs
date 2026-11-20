//! Pathological C++ input and a slow legitimate file, driven through the real
//! first-pass + serial-retry path (`extract_all_with_budgets`).
//!
//! `stress_hardening` feeds a pathological input to the parser with one direct
//! budget. That proves the parser honours a deadline; it does not prove the
//! *retry* is bounded, which is the other surface: a refusal in the parallel
//! pass is re-run under a larger budget, and a retry that did not honour its
//! own budget would turn the per-file bound back into the unbounded cost.
//!
//! "Slow" is injected through [`ParseBudgets`], the existing seam: a zero
//! first-pass budget refuses every file in the parallel pass exactly as a
//! contended machine does, without depending on the machine's load.
//!
//! Which C++ shape is pathological has changed. The 2,000-nested-brace input
//! that once cost 131 s now parses in milliseconds on `tree-sitter-cpp` 0.23.4,
//! so it is the "formerly pathological, now recovered whole" case here. The
//! shapes that are still superlinear are long method chains and deep call
//! nests (see `DEFAULT_PARSE_BUDGET` for measurements); an 8,000-call method
//! chain is the pathological case, because it overruns a 1.5 s retry budget by
//! a wide margin in both debug and release builds.

use devmap_extract::{
    extract_all_with_budgets, extract_file_with_budget, Extraction, FileRef, ParseBudgets,
    ParseOutcome, BUDGET_REFUSAL_PREFIX,
};
use std::time::{Duration, Instant};

/// The retry budget the pathological case must run into.
const RETRY: Duration = Duration::from_millis(1_500);

/// How far past the retry budget a refused file may run before the retry is
/// not a bound. The stop is polled, so it overshoots a little; 10x the budget
/// is far below the tens of seconds the input costs unbounded.
const SLACK: u32 = 10;

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

/// Once 131 s; now milliseconds. Kept as the recovered-whole case.
fn formerly_pathological_braces() -> String {
    format!("{}{}", "{".repeat(2_000), "}".repeat(2_000))
}

/// Still superlinear in `tree-sitter-cpp` 0.23.4: 8,000 chained calls, 32 KB.
fn method_chain() -> String {
    format!("int main() {{ a{}; }}\n", ".b()".repeat(8_000))
}

fn reason(extraction: &Extraction) -> &str {
    match &extraction.parse_outcome {
        ParseOutcome::Failed { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Byte-for-byte what an unhurried extraction of the same file yields: a
/// retry that returned a hollow or truncated extraction cannot equal it.
fn assert_equals_unhurried(recovered: &Extraction, path: &str, source: &str) {
    let unhurried = extract_file_with_budget(path, source, Duration::from_secs(60));
    assert!(!unhurried.is_parse_failure(), "{path}: baseline refused");
    assert_eq!(
        serde_json::to_string(recovered).unwrap(),
        serde_json::to_string(&unhurried).unwrap(),
        "{path}: the recovered extraction differs from an unhurried one"
    );
}

#[test]
fn a_slow_legitimate_file_and_the_formerly_pathological_braces_are_recovered_whole() {
    let legit = large_legitimate_python();
    let braces = formerly_pathological_braces();
    let files = [
        FileRef {
            path: "big.py",
            source: &legit,
        },
        FileRef {
            path: "braces.cpp",
            source: &braces,
        },
    ];
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::from_secs(30),
        retry_total: Duration::from_secs(120),
    };

    let started = Instant::now();
    let (out, report) = extract_all_with_budgets(&files, None, budgets, None);
    let elapsed = started.elapsed();

    // Both were refused in the parallel pass; that is the injected slowness.
    assert_eq!(report.first_pass_refused, 2, "{report:?}");
    assert_eq!(report.retried, 2, "{report:?}");
    assert_eq!(report.recovered, 2, "{report:?}");
    assert!(report.still_refused.is_empty(), "{report:?}");

    for extraction in &out {
        assert!(
            matches!(
                extraction.parse_outcome,
                ParseOutcome::Clean | ParseOutcome::Partial { .. }
            ),
            "{}: {:?}",
            extraction.file_path,
            extraction.parse_outcome
        );
        assert!(
            !extraction.symbols.is_empty(),
            "{}: recovered but hollow",
            extraction.file_path
        );
    }
    assert_eq!(
        out[0]
            .symbols
            .iter()
            .filter(|s| s.name.starts_with("fn_"))
            .count(),
        3_000,
        "a recovered file must carry every declaration"
    );
    assert!(!out[1].calls.is_empty() || !out[1].symbols.is_empty());
    assert_equals_unhurried(&out[0], "big.py", &legit);
    assert_equals_unhurried(&out[1], "braces.cpp", &braces);
    assert!(elapsed < Duration::from_secs(60), "took {elapsed:?}");
}

#[test]
fn a_pathological_chain_is_refused_again_by_the_retry_named_and_bounded() {
    let chain = method_chain();
    let files = [FileRef {
        path: "chain.cpp",
        source: &chain,
    }];
    let budgets = ParseBudgets {
        first_pass: Duration::ZERO,
        retry: RETRY,
        retry_total: Duration::from_secs(120),
    };

    let started = Instant::now();
    let (out, report) = extract_all_with_budgets(&files, None, budgets, None);
    let elapsed = started.elapsed();

    assert_eq!(report.first_pass_refused, 1, "{report:?}");
    assert_eq!(report.retried, 1, "{report:?}");
    assert_eq!(report.recovered, 0, "{report:?}");
    assert!(out[0].is_budget_refusal(), "{:?}", out[0].parse_outcome);
    let why = reason(&out[0]);
    assert!(why.starts_with(BUDGET_REFUSAL_PREFIX), "{why}");
    assert!(why.contains("refused again by the serial retry"), "{why}");
    assert_eq!(report.still_refused, vec!["chain.cpp"], "{report:?}");
    assert_eq!(report.still_refused_total, 1);

    // It ran into the retry budget (the retry really ran) and stopped near it.
    assert!(
        elapsed >= RETRY,
        "the retry did not run to its budget: {elapsed:?}"
    );
    assert!(
        elapsed < RETRY * SLACK,
        "the retry did not honour its {RETRY:?} budget; took {elapsed:?}"
    );
}

#[test]
fn a_refused_retry_with_no_budget_at_all_is_named_and_returns_promptly() {
    let braces = formerly_pathological_braces();
    let files = [FileRef {
        path: "braces.cpp",
        source: &braces,
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
    assert!(reason(&out[0]).contains("refused again by the serial retry"));
    assert_eq!(report.still_refused, vec!["braces.cpp"], "{report:?}");
    assert!(
        elapsed < Duration::from_secs(10),
        "a refused file must cost almost nothing; took {elapsed:?}"
    );
}
