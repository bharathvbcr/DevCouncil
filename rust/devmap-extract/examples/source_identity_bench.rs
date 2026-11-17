//! Throughput of `source_identity` against plain `fnv1a64` on a 1 MiB input
//! (`MAX_SOURCE_BYTES`, the largest file the index admits).
//!
//! The length fold adds one XOR and two multiplies per file, so the expected
//! result is a gap inside the noise floor. This prints the A/A noise floor
//! first (`fnv1a64` against itself), then interleaved A/B rounds, and reads the
//! A/B gap against that spread. Measurement only, no assertions. Run alone on
//! a quiet host, release, single thread:
//!
//! ```sh
//! cargo run --release -p devmap-extract --example source_identity_bench
//! ```
//!
//! Dependency-free by design, like `budget_probe`: no benchmark harness
//! dependency. Recorded runs, with commit, machine and load average, are in
//! the commit message that adds or changes a number here.

use std::hint::black_box;
use std::time::Instant;

use devmap_extract::{fnv1a64, source_identity_bytes};

const SIZE: usize = 1 << 20;
const ROUNDS: usize = 31;
/// Repetitions inside one timed sample, so a sample is long enough to read.
const REPS: usize = 8;

/// Deterministic text-like bytes from a fixed LCG.
fn input() -> Vec<u8> {
    let mut state: u64 = 0x2545F4914F6CDD1D;
    (0..SIZE)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            b' ' + ((state >> 33) % 95) as u8
        })
        .collect()
}

fn sample(f: fn(&[u8]) -> u64, bytes: &[u8]) -> f64 {
    let started = Instant::now();
    let mut acc = 0u64;
    for _ in 0..REPS {
        acc ^= f(black_box(bytes));
    }
    black_box(acc);
    started.elapsed().as_secs_f64() / REPS as f64
}

fn min_median(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

fn mib_per_s(secs: f64) -> f64 {
    SIZE as f64 / (1 << 20) as f64 / secs
}

/// Interleaved ABAB rounds; returns `(a, b)` per-call seconds in round order.
fn rounds(a: fn(&[u8]) -> u64, b: fn(&[u8]) -> u64, bytes: &[u8]) -> (Vec<f64>, Vec<f64>) {
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        sa.push(sample(a, bytes));
        sb.push(sample(b, bytes));
    }
    (sa, sb)
}

fn report(label: &str, a: Vec<f64>, b: Vec<f64>) {
    let b_faster = a.iter().zip(&b).filter(|(x, y)| y < x).count();
    let (amin, amed) = min_median(a);
    let (bmin, bmed) = min_median(b);
    println!(
        "{label}: A min {:.3} ms ({:.0} MiB/s) med {:.3} ms | B min {:.3} ms ({:.0} MiB/s) med {:.3} ms | min gap {:+.2}% | B faster in {b_faster}/{ROUNDS} rounds",
        amin * 1e3,
        mib_per_s(amin),
        amed * 1e3,
        bmin * 1e3,
        mib_per_s(bmin),
        bmed * 1e3,
        (bmin / amin - 1.0) * 100.0,
    );
}

fn main() {
    let bytes = input();
    assert_eq!(bytes.len(), SIZE);
    // The two arms must differ in value (the fold ran) before time is read.
    assert_ne!(fnv1a64(&bytes), source_identity_bytes(&bytes));
    println!("input {SIZE} bytes, {ROUNDS} interleaved rounds x {REPS} reps, single thread");
    // Warm the cache and the clock.
    for _ in 0..4 {
        sample(fnv1a64, &bytes);
        sample(source_identity_bytes, &bytes);
    }
    let (a1, a2) = rounds(fnv1a64, fnv1a64, &bytes);
    report("A/A noise floor (fnv1a64 vs fnv1a64)", a1, a2);
    let (a, b) = rounds(fnv1a64, source_identity_bytes, &bytes);
    report("A/B (fnv1a64 vs source_identity_bytes)", a, b);
}
