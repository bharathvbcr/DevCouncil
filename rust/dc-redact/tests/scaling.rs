//! A long line must cost time proportional to its length.
//!
//! The ratio, not a wall-clock bound, is what proves linearity: a bound loose
//! enough for a slow CI machine passes a quadratic scan on a short input.
//! Each shape is measured at 1x and 8x a base size (100 KB in a release build,
//! 10 KB in a debug one), min of several runs, and the cost per byte may grow
//! at most 3x between them. Linear scanning keeps it at 1x; quadratic scanning
//! grows it 8x. The bound sits between the two with margin on both sides.
//!
//! Measured on the unmodified scan at 1x, 2x, 4x, 8x and 16x (release, min of
//! 15): every doubling costs 1.9x to 2.2x, so 16x the input cost 16x to 18.6x
//! the time. The scan is linear. What 4x-for-5x measured on a CI runner was the
//! cache: 400 KB of input and as much again of redacted output no longer sit
//! where 100 KB did, which raises the cost per byte by tens of percent and
//! leaves a 25% allowance of noise at the bound. A per-byte allowance of 3x is
//! well clear of that and still 2.7x below what a quadratic scan costs.

use dc_redact::{contains_secret, redact_secrets};
use std::time::{Duration, Instant};

/// How many times the large input is longer than the small one.
const GROWTH: u32 = 8;

/// How much the cost per byte may grow across `GROWTH`. Linear is 1, quadratic
/// is `GROWTH` (8).
const PER_BYTE_ALLOWANCE: u32 = 3;

/// The cheapest of `runs` timings of `work` on each input, taken in
/// alternation: small, large, small, large.
///
/// The two inputs used to be timed one after the other, nine runs of one and
/// then nine of the other. A burst of load that covered the second phase — a
/// loaded CI host, other test binaries — then inflated only the large input's
/// time, and the ratio read 7x for a scan that is linear (28 ms against 207 ms
/// at a load average of 150; 18 ms against 80 ms, a ratio of 4.4, on a quiet
/// machine). Alternating means any burst reaches both inputs, and the minimum
/// of each keeps the runs the burst missed. It is still wall-clock time: the
/// scan is one thread that does not wait, so the quiet runs are its CPU cost.
fn best_pair(small: &str, large: &str, runs: usize, work: &dyn Fn(&str)) -> (Duration, Duration) {
    let time = |input: &str| {
        let started = Instant::now();
        work(input);
        started.elapsed()
    };
    let (mut a, mut b) = (Duration::MAX, Duration::MAX);
    for _ in 0..runs {
        a = a.min(time(small));
        b = b.min(time(large));
    }
    (a, b)
}

fn scan(line: &str) {
    let _ = contains_secret(line);
    let _ = redact_secrets(line);}

/// `GROWTH` times the input may cost at most `PER_BYTE_ALLOWANCE` times as much
/// per byte. A floor keeps timer noise on a fast scan from reading as a ratio.
fn is_linear(a: Duration, b: Duration) -> bool {
    b <= Duration::from_millis(20).max(a * (GROWTH * PER_BYTE_ALLOWANCE))
}

/// Every assignment- and flag-shaped credential whose literal is judged at each
/// separator of one long line.
#[test]
fn repeated_credential_assignments_scale_linearly() {
    let shapes: [(&str, &str); 15] = [
        // Every `:` is a credential key's, and the type after it runs to the
        // end of the line: the annotation check reads a bounded prefix.
        ("token: type run", "token:a:"),
        ("password=", "password=Ab12345678 "),
        ("token=", "token=Ab12345678 "),
        ("PGPASSWORD=", "PGPASSWORD=Ab12345678 "),
        ("--password=", "--password=Ab12345678 "),
        (
            "aws_secret_access_key=",
            "aws_secret_access_key=Ab12345678 ",
        ),
        ("password= bare", "password="),
        ("password= unquoted run", "password=Ab12345678Ab12345678,"),
        ("quoted", "password=\"Ab12345678\" "),
        ("curl -u", "curl -u a:Ab12345678 "),
        ("curl bare", "curl -u "),
        ("docker login", "docker login -p Ab12345678 "),
        ("--password value", "--password Ab12345678 "),
        ("--password flags", "--password --password "),
        ("netrc", "machine m login l password Ab12345678 "),
    ];
    // A debug build scans some twenty times slower, so it measures a tenth of
    // the release size: the ratio is the same.
    let scale = if cfg!(debug_assertions) {
        10_000
    } else {
        100_000
    };
    for (name, unit) in shapes {
        let small = unit.repeat(scale / unit.len());
        let large = unit.repeat(GROWTH as usize * scale / unit.len());
        let (a, b) = best_pair(&small, &large, 7, &scan);
        eprintln!(
            "{name}: {}B {a:?}  {}B {b:?}  ratio {:.1} (linear is {GROWTH})",
            small.len(),
            large.len(),
            b.as_secs_f64() / a.as_secs_f64().max(1e-9)
        );
        assert!(
            is_linear(a, b),
            "{name}: {GROWTH}x the input cost more than {}x the time: {a:?} -> {b:?}",
            GROWTH * PER_BYTE_ALLOWANCE
        );
    }
}

/// Busy-waits `nanos_per_unit * units` so a synthetic scan costs exactly what
/// its shape says, independent of the machine.
fn spin(units: usize, nanos_per_unit: u64) {
    let until = Instant::now() + Duration::from_nanos(nanos_per_unit * units as u64);
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

fn synthetic_inputs() -> (String, String) {
    (
        "x".repeat(25_000),
        "x".repeat(GROWTH as usize * 25_000),
    )
}

/// The measurement still catches what it exists to catch: a quadratic scan
/// reads as superlinear, a linear one does not.
#[test]
fn the_measurement_separates_a_quadratic_scan_from_a_linear_one() {
    let (small, large) = synthetic_inputs();
    let linear = |s: &str| spin(s.len(), 200);
    let quadratic = |s: &str| spin(s.len() * s.len() / 25_000, 200);
    let (a, b) = best_pair(&small, &large, 5, &linear);
    assert!(
        is_linear(a, b),
        "a linear scan was called superlinear: {a:?} -> {b:?}"
    );
    let (a, b) = best_pair(&small, &large, 5, &quadratic);
    assert!(
        !is_linear(a, b),
        "a quadratic scan passed as linear: {a:?} -> {b:?}"
    );
}

/// A scan whose cost per byte grows by the 25% a cache boundary adds, the
/// shape that failed on macos-latest at 5.0x for 4x, is still linear; one that
/// grows as n^1.7 is not.
#[test]
fn a_cache_sized_step_in_cost_per_byte_is_linear_and_a_slow_climb_is_not() {
    let (small, large) = synthetic_inputs();
    let cache_step = |s: &str| {
        let per_byte = if s.len() > small.len() { 250 } else { 200 };
        spin(s.len(), per_byte);
    };
    let (a, b) = best_pair(&small, &large, 5, &cache_step);
    assert!(
        is_linear(a, b),
        "a 25% step in cost per byte was called superlinear: {a:?} -> {b:?}"
    );
    // n^1.7 grows the cost per byte 8^0.7 = 4.3x over this range.
    let n_to_1_7 = |s: &str| {
        let units = (s.len() as f64).powf(1.7) / (25_000f64).powf(0.7);
        spin(units as usize, 200);
    };
    let (a, b) = best_pair(&small, &large, 5, &n_to_1_7);
    assert!(
        !is_linear(a, b),
        "an n^1.7 scan passed as linear: {a:?} -> {b:?}"
    );
}

/// A burst of load that lasts as long as the large phase of a sequential
/// measurement must not make a linear scan look superlinear.
#[test]
fn a_burst_of_load_over_one_phase_does_not_fail_a_linear_scan() {
    let (small, large) = synthetic_inputs();
    let calls = std::cell::Cell::new(0usize);
    // Calls 9..18 are slow: in a sequential measurement that is exactly the
    // nine runs of the large input.
    let noisy = |s: &str| {
        let n = calls.get();
        calls.set(n + 1);
        let burst = if (9..18).contains(&n) { 6 } else { 1 };
        spin(s.len(), 200 * burst);
    };
    let (a, b) = best_pair(&small, &large, 9, &noisy);
    assert!(
        is_linear(a, b),
        "a load burst read as superlinear: {a:?} -> {b:?}"
    );
}
