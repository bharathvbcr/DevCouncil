//! Does a smaller page cache, or mmap, change what a read-only MCP connection
//! costs? The comparison is stated before it is run, so it can come out either
//! way.
//!
//! Run (on a COPY of a large store; never a live one):
//! ```text
//! cargo run --release --example mcp_reader_memory_bench -p devmap-serve -- \
//!     --driver <copy.sqlite> <rounds> <target> [<target>...]
//! ```
//!
//! # Arms
//!
//! * `baseline` — today's read-only MCP connection: `cache_size = -65536`, no mmap.
//! * `cache16` — `cache_size = -16384` only.
//! * `cache16-mmap256` — `cache_size = -16384` plus `mmap_size = 256 MiB`.
//!
//! # Method
//!
//! Each sample is one fresh process: it opens the store with the arm's settings,
//! makes one cold `status` and one cold `impact` per target (recorded, not
//! compared), then `WARM_ROUNDS` rounds of `status` and `impact` over the
//! targets, and finally asks `vmmap --summary` for its own physical footprint.
//! A process's warm figure is the median of its warm calls. The driver runs the
//! arms interleaved, rotating which arm goes first each round so a position in
//! the order is not an arm, and reports for each arm n, min, median and max of
//! those per-process figures, with the host load average at start and end.
//!
//! A difference smaller than the max-min spread of an arm's own repeats is not
//! reported as a gain. Warm calls are served from the cached edge index, so
//! SQLite page-cache settings are not expected to move them; the footprint and
//! the cold calls are where an arm could show.

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::{ReaderMemory, Store};
use serde_json::{json, Value};

const WARM_ROUNDS: usize = 12;

const ARMS: [(&str, ReaderMemory); 3] = [
    ("baseline", ReaderMemory::CURRENT),
    (
        "cache16",
        ReaderMemory {
            cache_kib: 16_384,
            mmap_bytes: 0,
            tolerate_full_text_damage: false,
        },
    ),
    (
        "cache16-mmap256",
        ReaderMemory {
            cache_kib: 16_384,
            mmap_bytes: 256 * 1024 * 1024,
            tolerate_full_text_damage: false,
        },
    ),
];

fn arm_named(name: &str) -> ReaderMemory {
    ARMS.iter()
        .find(|(arm, _)| *arm == name)
        .unwrap_or_else(|| panic!("unknown arm {name}"))
        .1
}

async fn ask(slot: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Duration {
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    let started = Instant::now();
    let response = handle_line(slot, &frame.to_string())
        .await
        .expect("a response frame");
    let elapsed = started.elapsed();
    assert_eq!(
        response["result"]["isError"],
        json!(false),
        "{tool} failed: {response}"
    );
    elapsed
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// `Physical footprint` of this process in MiB, from `vmmap --summary`.
fn physical_footprint_mib() -> f64 {
    let output = Command::new("vmmap")
        .args(["--summary", &std::process::id().to_string()])
        .output()
        .expect("vmmap runs");
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.starts_with("Physical footprint:"))
        .unwrap_or_else(|| panic!("no footprint line in vmmap output: {text}"));
    let figure = line.split_whitespace().nth(2).expect("a figure");
    let (number, unit) = figure.split_at(figure.len() - 1);
    let number: f64 = number.parse().expect("a number");
    match unit {
        "K" => number / 1024.0,
        "M" => number,
        "G" => number * 1024.0,
        other => panic!("unexpected vmmap unit {other} in {line}"),
    }
}

async fn child(arm: &str, store: &str, targets: &[String]) {
    let path = std::path::PathBuf::from(store);
    let memory = arm_named(arm);
    let opened = Store::open_read_only_with(&path, memory).expect("open the store copy");
    let slot = Arc::new(StoreSlot::ready(&path, Arc::new(opened)));

    let cold_status = ask(&slot, "devmap_status", json!({})).await;
    let mut cold_impact = Duration::ZERO;
    for target in targets {
        cold_impact += ask(&slot, "devmap_impact", json!({"target": target})).await;
    }
    let mut warm_status = Vec::new();
    let mut warm_impact = Vec::new();
    for _ in 0..WARM_ROUNDS {
        warm_status.push(millis(ask(&slot, "devmap_status", json!({})).await));
        for target in targets {
            warm_impact.push(millis(
                ask(&slot, "devmap_impact", json!({"target": target})).await,
            ));
        }
    }
    println!(
        "{}",
        json!({
            "arm": arm,
            "cold_status_ms": millis(cold_status),
            "cold_impact_ms": millis(cold_impact),
            "warm_status_ms": median(&mut warm_status),
            "warm_impact_ms": median(&mut warm_impact),
            "footprint_mib": physical_footprint_mib(),
        })
    );
}

fn load_average() -> String {
    let output = Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .expect("sysctl runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn spread(values: &mut [f64]) -> String {
    values.sort_by(f64::total_cmp);
    format!(
        "min {:>9.2}  median {:>9.2}  max {:>9.2}",
        values[0],
        values[values.len() / 2],
        values[values.len() - 1]
    )
}

fn driver(store: &str, rounds: usize, targets: &[String]) {
    let exe = std::env::current_exe().expect("own path");
    println!("load before: {}", load_average());
    let mut samples: Vec<(String, Value)> = Vec::new();
    for round in 0..rounds {
        for offset in 0..ARMS.len() {
            let (arm, _) = ARMS[(round + offset) % ARMS.len()];
            let output = Command::new(&exe)
                .args(["--child", arm, store])
                .args(targets)
                .output()
                .expect("spawn a sample");
            assert!(
                output.status.success(),
                "sample failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let line = String::from_utf8_lossy(&output.stdout);
            samples.push((
                arm.to_string(),
                serde_json::from_str(line.lines().last().expect("a result line")).expect("json"),
            ));
        }
    }
    println!("load after:  {}", load_average());
    println!("rounds (n per arm) = {rounds}, warm rounds per sample = {WARM_ROUNDS}");
    for (arm, _) in ARMS {
        println!("\n{arm}");
        for key in [
            "warm_status_ms",
            "warm_impact_ms",
            "cold_status_ms",
            "cold_impact_ms",
            "footprint_mib",
        ] {
            let mut values: Vec<f64> = samples
                .iter()
                .filter(|(name, _)| name == arm)
                .map(|(_, sample)| sample[key].as_f64().expect("a number"))
                .collect();
            println!("  {key:<16} {}", spread(&mut values));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    match args.first().map(String::as_str) {
        Some("--child") => runtime.block_on(child(&args[1], &args[2], &args[3..])),
        Some("--driver") => driver(
            &args[1],
            args[2].parse().expect("a round count"),
            &args[3..],
        ),
        _ => panic!("usage: --driver <store> <rounds> <target>..."),
    }
}
