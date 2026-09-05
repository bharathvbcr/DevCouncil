//! Measures what an agent actually pays per code-intelligence question.
//!
//! Run:
//! ```text
//! cargo run --release --example mcp_bench -p devmap-serve -- <path/to/devmap.sqlite>
//! ```
//!
//! # What is being compared
//!
//! Three ways the same question reaches the same query engine:
//!
//! * **in-process** — `mcp::handle_line` against a store this process holds open.
//!   This is what `devmap mcp` costs an agent host after the first call.
//! * **subprocess** — spawning the `devmap` binary per question. This is what the
//!   Python seam does whenever no daemon socket is live
//!   (`DevMapClient._request`), and it is the path most sessions are actually on.
//! * **cold-open** — opening the store, asking once, dropping it. Isolates how
//!   much of the subprocess number is process startup versus store open, so the
//!   difference is attributed rather than guessed at.
//!
//! # Why it is written this way
//!
//! An `examples/` target, not a `benches/` one: `benches/` wants a harness crate
//! and no new dependency was authorized for this. `std::time::Instant` with
//! reported quantiles is enough to answer "is this 10x or 1.1x", which is the
//! question. It is not enough to detect a 3% regression, and this file does not
//! claim otherwise — it prints N and the spread so the reader can judge.
//!
//! Medians and p95, never means: process spawn times are right-skewed by
//! scheduler noise, and a mean over 50 samples with one 200ms outlier reports a
//! number that occurred zero times.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;

/// Discarded before measuring. The first call pays page-cache misses, SQLite
/// statement preparation and lazy store open; including it measures the setup,
/// which is paid once, as though it were paid per question.
const WARMUP: usize = 5;
const SAMPLES: usize = 50;
/// Process spawn is ~1000x more expensive than an in-process call, so a matching
/// sample count would dominate the runtime for no extra confidence.
const SPAWN_SAMPLES: usize = 15;

struct Stats {
    label: String,
    samples: Vec<Duration>,
}

impl Stats {
    fn quantile(&self, q: f64) -> Duration {
        // Sorted copy per call: SAMPLES is small and this runs after timing, so
        // the cost is irrelevant and the alternative is a mutable borrow that
        // makes the call sites awkward.
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let index = ((sorted.len() - 1) as f64 * q).round() as usize;
        sorted[index]
    }

    fn report(&self) -> String {
        format!(
            "{:<26} n={:<4} min={:>10.3?}  p50={:>10.3?}  p95={:>10.3?}  max={:>10.3?}",
            self.label,
            self.samples.len(),
            self.quantile(0.0),
            self.quantile(0.5),
            self.quantile(0.95),
            self.quantile(1.0),
        )
    }
}

fn time_it(label: &str, iterations: usize, mut body: impl FnMut()) -> Stats {
    for _ in 0..WARMUP {
        body();
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        body();
        samples.push(started.elapsed());
    }
    Stats {
        label: label.to_string(),
        samples,
    }
}

fn main() -> anyhow::Result<()> {
    let db = std::env::args().nth(1).ok_or_else(|| {
        anyhow::anyhow!(
            "usage: mcp_bench <path/to/devmap.sqlite>\n\n\
             Point this at a store you built and let settle. Do NOT point it at a \
             repository's own live store: a generation still being swept measures the \
             sweep, not the query."
        )
    })?;

    // The store must be settled before a single number is taken. A store with no
    // nodes makes every query trivially fast and the comparison meaningless; a
    // store with pending work measures the writer holding the mutex. Both have
    // produced wrong numbers in this repository before, which is why this is an
    // assertion and not a note in the output.
    let store = Store::open_existing(&db)?
        .ok_or_else(|| anyhow::anyhow!("no devmap store at {db} — build one first"))?;
    let status = store.status("bench")?;
    anyhow::ensure!(
        status.node_count > 0,
        "store at {db} has node_count = 0; every query would return nothing and the \
         comparison would measure an empty index"
    );
    anyhow::ensure!(
        status.pending_count == 0,
        "store at {db} has {} files pending; a mid-sweep store measures the sweep, not \
         the query. Let it settle and re-run.",
        status.pending_count
    );
    println!(
        "store: {db}\n  generation={:?} nodes={} edges={} pending={}\n",
        status.latest_generation, status.node_count, status.edge_count, status.pending_count
    );

    let runtime = tokio::runtime::Runtime::new()?;
    let slot = Arc::new(StoreSlot::ready(&db, Arc::new(store)));

    // One representative call per shape: a text search, a reverse walk, and the
    // composed multi-target question the seam was built to collapse.
    let calls = [
        (
            "status",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_status","arguments":{}}}"#,
        ),
        (
            "search",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_search","arguments":{"query":"build"}}}"#,
        ),
        (
            "impact",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_impact","arguments":{"target":"main"}}}"#,
        ),
        (
            "tools/list",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ),
    ];

    let mut results = Vec::new();

    for (name, frame) in calls {
        // Asserted, not assumed: a benchmark of a call that silently errors
        // measures the error path. This has happened here before — a fallback
        // was measured and reported as the kernel.
        let probe = runtime
            .block_on(handle_line(&slot, frame))
            .ok_or_else(|| anyhow::anyhow!("{name} produced no response frame"))?;
        if let Some(result) = probe.get("result") {
            anyhow::ensure!(
                result.get("isError") != Some(&serde_json::Value::Bool(true)),
                "{name} returns a tool error, so this would benchmark the error path: {}",
                result["content"][0]["text"]
            );
        }
        anyhow::ensure!(
            probe.get("error").is_none(),
            "{name} returns a protocol error: {}",
            probe["error"]
        );

        results.push(time_it(&format!("in-process {name}"), SAMPLES, || {
            let _ = runtime.block_on(handle_line(&slot, frame));
        }));
    }

    // The subprocess path, measured against the same store. `devmap` is looked up
    // beside this example's own build output, so the number is for the binary
    // just built rather than whatever is on PATH.
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("../devmap")))
        .filter(|p| p.exists());

    match binary {
        Some(binary) => {
            results.push(time_it("subprocess status", SPAWN_SAMPLES, || {
                let output = std::process::Command::new(&binary)
                    .args(["--db", &db, "status"])
                    .output()
                    .expect("spawn devmap");
                assert!(
                    output.status.success(),
                    "devmap status failed, so this measures the failure path: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }));
        }
        None => {
            // Named, not skipped silently. A comparison that quietly loses its
            // baseline reports the remaining column as though it were the whole
            // answer.
            println!(
                "NOTE: devmap binary not found beside this example; the subprocess \
                 comparison was NOT measured and no spawn number is reported below.\n"
            );
        }
    }

    results.push(time_it("cold-open + status", SPAWN_SAMPLES, || {
        let store = Store::open_existing(&db)
            .expect("open")
            .expect("store exists");
        let _ = store.status("bench").expect("status");
    }));

    println!("{:-<100}", "");
    for stats in &results {
        println!("{}", stats.report());
    }
    println!("{:-<100}", "");
    println!(
        "\nWARMUP={WARMUP} discarded per row. Quantiles over the sample counts shown.\n\
         p50 is the number to quote; p95 is what a user notices."
    );
    Ok(())
}
