//! `diskwatch <pid> <seconds>` — sample one running process's disk-write
//! counters every half second for up to `<seconds>` (or until it exits) and
//! judge the worst 30-second window against the soak's write budget.
//!
//! This is the measurement the 549.76 GB `devmap` disk-writes report asked
//! for: point it at a long-running `devmap serve` (or a build loop's pid) and
//! it prints `elapsed_s,storage_bytes,logical_bytes` every five seconds, then
//! one verdict line.
//!
//! `SOAK_WRITE_BUDGET_MIB` replaces the 40 MiB/s rate.
//!
//! Exit codes: 0 within budget · 1 over budget, or the budget could not be
//! judged (NOT RUN) · 2 usage.

use std::time::{Duration, Instant};

use devmap_soak::diskio::{judge, mib_per_s, Budget, Sampler, Verdict};

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(pid), Some(seconds), 2) = (
        args.first().and_then(|a| a.parse::<u32>().ok()),
        args.get(1)
            .and_then(|a| a.parse::<u64>().ok())
            .filter(|s| (1..=7 * 24 * 3600).contains(s)),
        args.len(),
    ) else {
        eprintln!("usage: diskwatch <pid> <seconds 1..604800>");
        return 2;
    };
    let mut budget = Budget::default();
    if let Ok(raw) = std::env::var("SOAK_WRITE_BUDGET_MIB") {
        match raw.trim().parse::<u64>() {
            Ok(mib) if mib > 0 => budget.rate = mib << 20,
            _ => {
                println!("DISKWATCH FAIL: SOAK_WRITE_BUDGET_MIB must be a positive integer");
                return 1;
            }
        }
    }
    let sampler = match Sampler::start(pid) {
        Ok(sampler) => sampler,
        Err(why) => {
            println!("DISKWATCH NOT RUN: {why}");
            return 1;
        }
    };
    println!("elapsed_s,storage_bytes,logical_bytes");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut printed = 0usize;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(5));
        let readings = sampler.readings();
        for reading in readings.iter().skip(printed).step_by(10) {
            println!(
                "{:.1},{},{}",
                reading.at.as_secs_f64(),
                reading.counters.storage_bytes,
                reading.counters.logical_bytes
            );
        }
        printed = readings.len();
        if dc_proc::diskio::write_counters(pid).is_err() {
            break;
        }
    }
    let (readings, ended) = sampler.finish();
    if let Some(why) = ended {
        println!("note: {why}");
    }
    let total = |f: fn(&dc_proc::diskio::WriteCounters) -> u64| {
        readings
            .last()
            .zip(readings.first())
            .map_or(0, |(l, f0)| f(&l.counters).saturating_sub(f(&f0.counters)))
    };
    println!(
        "total over {:.0} s: storage {} bytes, logical {} bytes",
        readings.last().map_or(0.0, |r| r.at.as_secs_f64()),
        total(|c| c.storage_bytes),
        total(|c| c.logical_bytes)
    );
    match judge(&readings, budget) {
        Verdict::Within(w) => {
            println!(
                "DISKWATCH OK: worst {:?} window {:.2} MiB/s ({} bytes, {:.0}..{:.0} s), budget {} MiB/s",
                budget.window,
                mib_per_s(w.rate),
                w.bytes,
                w.from.as_secs_f64(),
                w.to.as_secs_f64(),
                budget.rate >> 20
            );
            0
        }
        Verdict::Over(w) => {
            println!(
                "DISKWATCH FAIL: worst {:?} window {:.2} MiB/s ({} bytes, {:.0}..{:.0} s), over the {} MiB/s budget",
                budget.window,
                mib_per_s(w.rate),
                w.bytes,
                w.from.as_secs_f64(),
                w.to.as_secs_f64(),
                budget.rate >> 20
            );
            1
        }
        Verdict::NotRun(why) => {
            println!("DISKWATCH NOT RUN: {why}");
            1
        }
    }
}
