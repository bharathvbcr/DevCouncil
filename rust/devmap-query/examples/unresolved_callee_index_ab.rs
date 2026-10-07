//! A/B `idx_unresolved_rows_callee`: what `impact` pays without it.
//!
//! `impact` lists the unresolved call sites that name its target
//! (`Store::unresolved_sites_naming`, `WHERE u.callee_name IN (…)`). Schema v21
//! dropped the only index on `unresolved_rows.callee_name` on the grounds that
//! no production statement filtered on it; this reader came later, and
//! `EXPLAIN QUERY PLAN` shows it scanning the whole ledger on every call.
//!
//! The two sides are copies of one store, made here, identical except for the
//! index: `VACUUM INTO` twice, then the index dropped from one and created on
//! the other. Both are opened read-only, which never migrates, so neither side
//! can quietly become the other. Every target's answer is compared across the
//! two before any time is reported — a faster wrong answer is not a result.
//!
//! Rounds are interleaved and alternate which side goes first, and the report
//! is min and p50: sequential before/after runs on this machine drifted by more
//! than the effects being measured. Run:
//!
//! ```sh
//! cargo run --release -p devmap-query --example unresolved_callee_index_ab -- \
//!     <devmap.sqlite> <target> [target ...]
//! ```
//!
//! ## Recorded results
//!
//! See the commit that re-added the index (schema v24) for the numbers this
//! printed on the DevCouncil store; they are repeated in `schema.rs` beside the
//! index, where the decision is.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_query::{Request, StoreQueryEngine};
use devmap_store::Store;
use rusqlite::{Connection, OpenFlags};

const ROUNDS: usize = 21;

fn main() -> anyhow::Result<()> {
    let profile = if cfg!(debug_assertions) {
        "debug (not comparable with release numbers)"
    } else {
        "release"
    };
    const USAGE: &str = "usage: unresolved_callee_index_ab <devmap.sqlite> <target> [target ...]";
    let mut args = std::env::args().skip(1);
    let source = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!(USAGE))?);
    // Named by the caller: `impact` refuses an ambiguous name, so which names
    // are usable depends on the corpus.
    let targets: Vec<String> = args.collect();
    anyhow::ensure!(!targets.is_empty(), USAGE);

    let scratch = std::env::temp_dir().join(format!("devmap-callee-ab-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let without = scratch.join("without.sqlite");
    let with = scratch.join("with.sqlite");
    let outcome = run(profile, &source, &without, &with, &targets);
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

fn run(
    profile: &str,
    source: &Path,
    without: &Path,
    with: &Path,
    targets: &[String],
) -> anyhow::Result<()> {
    copy_store(source, without)?;
    copy_store(source, with)?;
    Connection::open(without)?.execute_batch("DROP INDEX IF EXISTS idx_unresolved_rows_callee")?;
    let built = Instant::now();
    Connection::open(with)?.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_unresolved_rows_callee ON unresolved_rows(callee_name)",
    )?;
    let index_build = built.elapsed();
    let (size_without, size_with) = (
        std::fs::metadata(without)?.len(),
        std::fs::metadata(with)?.len(),
    );

    let store_without = Store::open_read_only(without)?;
    let store_with = Store::open_read_only(with)?;
    let rows: i64 = Connection::open_with_flags(without, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        .query_row("SELECT COUNT(*) FROM unresolved_rows", [], |row| row.get(0))?;
    let engine_without = StoreQueryEngine::new(&store_without);
    let engine_with = StoreQueryEngine::new(&store_with);

    let impact = |engine: &StoreQueryEngine<'_>, target: &str| {
        engine.impact(Request {
            query: target.to_string(),
            token_budget: devmap_query::Budget::DEPS,
            min_confidence: 0.0,
            max_depth: 3,
        })
    };
    // Equal answers first, and one untimed call per side so the rounds measure
    // warm caches rather than the transition into them.
    for target in targets {
        let a = serde_json::to_value(impact(&engine_without, target)?)?;
        let b = serde_json::to_value(impact(&engine_with, target)?)?;
        anyhow::ensure!(
            a == b,
            "the two sides answered `{target}` differently; refusing to time them"
        );
    }

    let mut times_without = Vec::with_capacity(ROUNDS);
    let mut times_with = Vec::with_capacity(ROUNDS);
    let time_side = |engine: &StoreQueryEngine<'_>| -> anyhow::Result<Duration> {
        let started = Instant::now();
        for target in targets {
            std::hint::black_box(impact(engine, target)?);
        }
        Ok(started.elapsed())
    };
    for round in 0..ROUNDS {
        if round % 2 == 0 {
            times_without.push(time_side(&engine_without)?);
            times_with.push(time_side(&engine_with)?);
        } else {
            times_with.push(time_side(&engine_with)?);
            times_without.push(time_side(&engine_without)?);
        }
    }

    println!("profile: {profile}");
    println!("store: {}", source.display());
    println!("unresolved_rows: {rows}");
    println!("targets ({}): {}", targets.len(), targets.join(", "));
    println!(
        "index: built in {:.1} ms; file {} -> {} bytes (+{})",
        index_build.as_secs_f64() * 1e3,
        size_without,
        size_with,
        size_with.saturating_sub(size_without)
    );
    let (min_a, p50_a) = summary(&mut times_without);
    let (min_b, p50_b) = summary(&mut times_with);
    println!(
        "impact x{} per round, {ROUNDS} interleaved rounds:",
        targets.len()
    );
    println!("  without index  min {min_a:>8.2} ms  p50 {p50_a:>8.2} ms");
    println!("  with index     min {min_b:>8.2} ms  p50 {p50_b:>8.2} ms");
    println!(
        "  per impact call, p50: {:.2} ms -> {:.2} ms",
        p50_a / targets.len() as f64,
        p50_b / targets.len() as f64
    );
    Ok(())
}

/// Copy a live store into a standalone file, WAL contents included.
fn copy_store(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])?;
    Ok(())
}

/// `(min, p50)` in milliseconds.
fn summary(samples: &mut [Duration]) -> (f64, f64) {
    samples.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    (ms(samples[0]), ms(samples[samples.len() / 2]))
}
