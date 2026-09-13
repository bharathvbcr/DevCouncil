//! The kernel's four phases over a thousand-file synthetic repository.
//!
//! **The budgets calibrate against this machine.** Every phase below used to
//! assert a bare `< 2.0s`. That is not a property of the code — it is a
//! property of whatever else the machine is doing, and on this one it is
//! routinely wrong: measured 2026-09-13 at load ~336, extraction of the same
//! 1,000 files took 2.37s and the suite went red with nothing whatsoever
//! changed in the kernel. A test that is red on a healthy tree catches
//! nothing; it teaches the reader to skip it.
//!
//! What is worth asserting is that the work stays *linear in the input*, which
//! is machine-independent. Each phase is therefore run twice — once over a
//! tenth of the corpus, once over all of it — and the full run is held to a
//! multiple of the extrapolated small run. An accidental O(n²) shows up
//! immediately at ten times the input; a machine under load moves both
//! measurements together and cancels out.
//!
//! The budget is floored at the old 2s constant, so this is never *stricter*
//! than what it replaces, only harder to fool.

use devmap_analyze::*;
use devmap_extract::*;
use devmap_resolve::*;
use devmap_store::*;
use std::time::{Duration, Instant};

/// How much slower than a perfectly linear extrapolation a phase may be.
///
/// Wide on purpose: it has to absorb one scheduler's worth of noise between two
/// measurements taken seconds apart on a shared machine. It still catches the
/// regression that matters — anything super-linear is off by far more than this
/// at ten times the input.
const SLACK: u32 = 4;

/// The budget never drops below the constant this replaced, so a very fast
/// calibration cannot manufacture a tighter bound than the original test had.
const FLOOR: Duration = Duration::from_secs(2);

/// A hang is not a slow machine. Nothing here should take a minute.
const CEILING: Duration = Duration::from_secs(120);

/// One phase's timings from a run of `files` modules.
struct Timings {
    extraction: Duration,
    resolution: Duration,
    analysis: Duration,
    database: Duration,
}

fn budget(calibration: Duration, scale: u32) -> Duration {
    (calibration * SLACK * scale).max(FLOOR)
}

fn assert_within(phase: &str, measured: Duration, calibration: Duration, scale: u32) {
    let allowed = budget(calibration, scale);
    assert!(
        measured < allowed,
        "{phase} took {measured:?} against a budget of {allowed:?} \
         ({SLACK}x a linear extrapolation of the {calibration:?} calibration \
         run, floored at {FLOOR:?}); that is super-linear, not a slow machine"
    );
    assert!(
        measured < CEILING,
        "{phase} took {measured:?}, past the {CEILING:?} hang ceiling"
    );
}

/// Run the whole pipeline over `num_files` synthetic modules.
fn pipeline(num_files: usize) -> anyhow::Result<(AnalysisSummary, ResolutionResult, Timings)> {
    let start_gen = Instant::now();
    let mut extractions = Vec::new();
    for i in 0..num_files {
        let path = format!("src/module_{}.py", i);
        let code = format!(
            "def fn_{}_a():\n    pass\n\ndef fn_{}_b():\n    fn_{}_a()\n",
            i, i, i
        );
        extractions.push(extract_file(&path, &code));
    }
    let extraction = start_gen.elapsed();

    let start_res = Instant::now();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let resolution_elapsed = start_res.elapsed();

    let start_ana = Instant::now();
    let analysis = analyze(&extractions, &resolution);
    let analysis_elapsed = start_ana.elapsed();

    let start_db = Instant::now();
    let store = Store::open_in_memory()?;
    let gen_id = store.save_generation(&extractions, &resolution, &analysis)?;
    let database = start_db.elapsed();
    assert_eq!(gen_id, 1);

    Ok((
        analysis,
        resolution,
        Timings {
            extraction,
            resolution: resolution_elapsed,
            analysis: analysis_elapsed,
            database,
        },
    ))
}

#[test]
fn test_large_repo_stress() -> anyhow::Result<()> {
    const CALIBRATION_FILES: usize = 100;
    const NUM_FILES: usize = 1000;
    const SCALE: u32 = (NUM_FILES / CALIBRATION_FILES) as u32;

    // The same work at a tenth of the size, on this machine, right now.
    println!("Calibrating on {CALIBRATION_FILES} files...");
    let (_, _, calib) = pipeline(CALIBRATION_FILES)?;
    println!(
        "  calibration: extract {:?} resolve {:?} analyze {:?} save {:?}",
        calib.extraction, calib.resolution, calib.analysis, calib.database
    );

    println!("Generating synthetic workload of {NUM_FILES} files...");
    let (analysis, resolution, timings) = pipeline(NUM_FILES)?;
    println!(
        "  full run:    extract {:?} resolve {:?} analyze {:?} save {:?}",
        timings.extraction, timings.resolution, timings.analysis, timings.database
    );

    assert_within("extraction", timings.extraction, calib.extraction, SCALE);
    assert_within("resolution", timings.resolution, calib.resolution, SCALE);
    assert_within("analysis", timings.analysis, calib.analysis, SCALE);
    assert_within("database save", timings.database, calib.database, SCALE);

    // Correctness, unchanged and unconditional: the budgets above say the work
    // scales, these say it is the right work.
    assert_eq!(analysis.total_files, NUM_FILES);
    assert_eq!(analysis.total_symbols, NUM_FILES * 3);
    // Per file: one `fn_i_b -> fn_i_a` call, plus one containment edge for each
    // of the two declared functions. Asserted by composition rather than as a
    // single magic total, so a regression names which kind changed.
    let calls = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .count();
    let contains = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Contains)
        .count();
    let references = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::References)
        .count();
    assert_eq!(calls, NUM_FILES, "one intra-file call per module");
    assert_eq!(contains, NUM_FILES * 2, "two declared functions per module");
    assert_eq!(analysis.total_edges, calls + contains + references);
    assert!(!analysis.communities.is_empty());

    for comm in &analysis.communities {
        assert!(comm.cohesion_score >= 0.0 && comm.cohesion_score <= 1.0);
    }
    let dead: std::collections::BTreeSet<_> = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt)
        .map(|report| report.symbol_name.as_str())
        .collect();
    let expected: std::collections::BTreeSet<_> = (0..NUM_FILES)
        .map(|index| format!("fn_{index}_b"))
        .collect();
    assert_eq!(dead, expected.iter().map(String::as_str).collect());

    Ok(())
}
