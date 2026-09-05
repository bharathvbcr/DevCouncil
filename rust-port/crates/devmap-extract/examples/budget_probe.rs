//! What `extract_treesitter_with_budget` actually costs against its stated
//! budget, on inputs chosen to attack different parts of the extractor.
//!
//! Measurement only — no assertions. `tests/budget_is_a_real_bound.rs` owns the
//! pass/fail property; this prints the ratios, which is what you want while
//! changing a walk. Run it with:
//!
//! ```sh
//! cargo run --release -p devmap-extract --example budget_probe
//! ```
//!
//! Release matters: a debug build's numbers are dominated by its own overhead
//! for the rows that run to completion. Recorded results, before and after the
//! 2026-09-05 quadratic-walk fix, are in `STATUS.md`; the worst row moved from
//! 174.69 s to 333 ms against a 200 ms budget.
//!
//! Dependency-free by design — this workspace has no benchmark harness
//! dependency and is not taking one.

use std::time::{Duration, Instant};

fn probe(label: &str, path: &str, lang: &str, source: &str, budget: Duration) {
    let started = Instant::now();
    let extraction =
        devmap_extract::treesitter::extract_treesitter_with_budget(path, lang, source, budget);
    let elapsed = started.elapsed();
    let ratio = elapsed.as_secs_f64() / budget.as_secs_f64();
    let outcome = match &extraction.parse_outcome {
        devmap_extract::model::ParseOutcome::Clean => "Clean".to_string(),
        devmap_extract::model::ParseOutcome::Partial { error_ranges } => {
            format!("Partial({} ranges)", error_ranges.len())
        }
        devmap_extract::model::ParseOutcome::Failed { .. } => "Failed".to_string(),
        devmap_extract::model::ParseOutcome::Fallback { .. } => "Fallback".to_string(),
    };
    println!(
        "{label:<28} budget={budget:>8.2?} elapsed={elapsed:>9.2?} ratio={ratio:>6.1}x \
         outcome={outcome:<18} symbols={}",
        extraction.symbols.len()
    );
}

fn main() {
    let budget = Duration::from_millis(200);

    // Deep brace nesting: parses fast, walks slowly. The documented case.
    let deep_cpp = format!("{}{}", "{".repeat(2000), "}".repeat(2000));
    probe("cpp deep braces x2000", "d.cpp", "cpp", &deep_cpp, budget);

    let deeper_cpp = format!("{}{}", "{".repeat(10_000), "}".repeat(10_000));
    probe(
        "cpp deep braces x10000",
        "d.cpp",
        "cpp",
        &deeper_cpp,
        budget,
    );

    // Go: `go_method_sets` re-walks the whole tree AFTER walk_tree returns,
    // with no deadline of its own.
    let mut go = String::from("package main\n");
    for i in 0..4000 {
        go.push_str(&format!(
            "type I{i} interface {{ M{i}() }}\ntype T{i} struct{{}}\nfunc (t T{i}) M{i}() {{}}\n"
        ));
    }
    probe("go many interfaces x4000", "m.go", "go", &go, budget);

    // Many ERROR nodes: `parse_outcome_of` walks the error tree, unbounded.
    let broken = "fn ((((".repeat(20_000);
    probe("rust many errors x20000", "b.rs", "rust", &broken, budget);
}
