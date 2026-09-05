//! What `extract_treesitter_with_budget` actually costs against its stated
//! budget, on inputs chosen to attack different parts of the extractor.
//!
//! Measurement only — no assertions. `tests/budget_is_a_real_bound.rs` owns the
//! pass/fail property; this prints the ratios, which is what you want while
//! changing a walk. Run it with:
//!
//! ```sh
//! cargo run --release -p devmap-extract --example budget_probe
//! # …plus per-language throughput over one or more real trees:
//! cargo run --release -p devmap-extract --example budget_probe -- <root> [<root> …]
//! ```
//!
//! Release matters: a debug build's numbers are dominated by its own overhead
//! for the rows that run to completion. Recorded results, before and after the
//! 2026-09-05 quadratic-walk fix, are in `STATUS.md`; the worst row moved from
//! 174.69 s to 333 ms against a 200 ms budget.
//!
//! Dependency-free by design — this workspace has no benchmark harness
//! dependency and is not taking one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

/// Per-file cost of `extract_file` over a real tree, grouped by language.
///
/// The budget rows above answer "does a hostile file blow the bound". This
/// answers the other half — what the ordinary corpus costs — which is the
/// number a change to the walk actually moves and the one a regression shows up
/// in first. Discovery is `collect_sources_with_report`, the same walk the build
/// uses, so the file set is the file set production sees.
///
/// p50 rather than a mean: extraction time per file is long-tailed (a 300 KB
/// generated `parser.c` is three orders of magnitude off the median), and a
/// mean over that mostly reports whether the tail file was in the sample.
/// Totals are printed beside it, so neither number stands alone.
fn throughput(roots: &[PathBuf]) {
    #[derive(Default)]
    struct Rows {
        seconds: Vec<f64>,
        rates: Vec<f64>,
        bytes: u64,
        total_seconds: f64,
    }

    let mut by_language: BTreeMap<&'static str, Rows> = BTreeMap::new();
    let mut files = 0usize;
    let started_all = Instant::now();

    for root in roots {
        let (sources, report) = match devmap_extract::collect_sources_with_report(root) {
            Ok(found) => found,
            Err(error) => {
                println!("corpus {root:?} could not be walked: {error}");
                continue;
            }
        };
        let refused = report.refusals().count();
        println!(
            "corpus {root:?}: {} source file(s), {refused} refused by discovery",
            sources.len()
        );
        for (path, source) in &sources {
            let language = devmap_extract::detect_language(Path::new(path));
            let started = Instant::now();
            let extraction = devmap_extract::extract_file(path, source);
            let elapsed = started.elapsed().as_secs_f64();
            // Read the result so the extraction cannot be optimized away.
            std::hint::black_box(extraction.symbols.len());
            let row = by_language.entry(language).or_default();
            row.seconds.push(elapsed);
            row.bytes += source.len() as u64;
            row.total_seconds += elapsed;
            if elapsed > 0.0 {
                row.rates.push(source.len() as f64 / elapsed / 1_048_576.0);
            }
            files += 1;
        }
    }
    let wall = started_all.elapsed();

    fn p50(values: &mut [f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.sort_by(|left, right| left.partial_cmp(right).unwrap());
        values[values.len() / 2]
    }

    println!();
    println!(
        "{:<14} {:>6} {:>10} {:>12} {:>12} {:>10}",
        "language", "files", "MiB", "p50 files/s", "p50 MiB/s", "total s"
    );
    for (language, row) in &mut by_language {
        let median_seconds = p50(&mut row.seconds);
        let files_per_second = if median_seconds > 0.0 {
            1.0 / median_seconds
        } else {
            f64::INFINITY
        };
        println!(
            "{language:<14} {:>6} {:>10.2} {:>12.0} {:>12.2} {:>10.3}",
            row.seconds.len(),
            row.bytes as f64 / 1_048_576.0,
            files_per_second,
            p50(&mut row.rates),
            row.total_seconds
        );
    }
    let total_bytes: u64 = by_language.values().map(|row| row.bytes).sum();
    // The aggregate is printed as the aggregate it is, never as a per-language
    // figure: one slow language and one fast one average to a number that
    // describes neither.
    println!(
        "{:<14} {files:>6} {:>10.2} {:>12} {:>12.2} {:>10.3}  (whole corpus, single-threaded)",
        "ALL",
        total_bytes as f64 / 1_048_576.0,
        "-",
        total_bytes as f64 / wall.as_secs_f64() / 1_048_576.0,
        wall.as_secs_f64()
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

    // Argv names the trees to measure. With none, the workspace this crate
    // lives in is a real mixed-language corpus and is measured instead of
    // nothing, so the example always prints a table.
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let roots = if roots.is_empty() {
        vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))]
    } else {
        roots
    };
    println!();
    throughput(&roots);
}
