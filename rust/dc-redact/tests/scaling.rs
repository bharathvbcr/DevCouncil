//! A long line must cost time proportional to its length.
//!
//! The ratio, not a wall-clock bound, is what proves linearity: a bound loose
//! enough for a slow CI machine passes a quadratic scan on a short input.
//! Each shape is measured at 100 KB and 1.6 MB (6 KB and 100 KB in a debug
//! build), min of several runs, and 16x the input may cost at most 32x the time
//! (linear is 16x; quadratic is 256x). The range is wide so that noise — a
//! loaded host, cache effects between a 100 KB and a 400 KB line, which took a
//! 4x range to 5.2x — is small against the margin, and a shape that fails is
//! measured once more with more runs before it counts: a failure needs both.

use dc_redact::{contains_secret, find_secret, redact_secrets};
use std::time::{Duration, Instant};

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
    let _ = find_secret(line);
    let _ = redact_secrets(line);
}

/// 16x the input may cost at most 32x the time (linear is 16x, quadratic 256x).
/// A floor keeps timer noise on a fast scan from reading as a ratio.
fn is_linear(a: Duration, b: Duration) -> bool {
    b <= Duration::from_millis(20).max(a * 32)
}

/// Linear on the first measurement, or on a second one with more runs.
fn measured_linear(small: &str, large: &str, work: &dyn Fn(&str)) -> (bool, Duration, Duration) {
    let (a, b) = best_pair(small, large, 3, work);
    if is_linear(a, b) {
        return (true, a, b);
    }
    let (a, b) = best_pair(small, large, 7, work);
    (is_linear(a, b), a, b)
}

/// Every assignment- and flag-shaped credential whose literal is judged at each
/// separator of one long line.
#[test]
fn repeated_credential_assignments_scale_linearly() {
    let shapes: [(&str, &str); 21] = [
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
        // Same-line joins: the line is scanned as written and, when it holds
        // a run of two or more quoted literals, once more as one joined copy.
        ("join plus", "\"a\" + "),
        ("join adjacent", "\"a\" "),
        ("join split key", "\"AKIA\" + \"IOSFODNN7EXAMPLE\" "),
        ("unclosed single quote", "'a "),
        ("raw hash unclosed", "r#\"a "),
        ("raw hash mismatched closer", "r##\"a\"# "),
    ];
    // 100 KB against 1.6 MB in a release build. A debug build scans some
    // twenty times slower, so it measures a sixteenth of that: the ratio is the
    // same.
    let scale = if cfg!(debug_assertions) {
        6_250
    } else {
        100_000
    };
    for (name, unit) in shapes {
        let small = unit.repeat(scale / unit.len());
        let large = unit.repeat(16 * scale / unit.len());
        let (linear, a, b) = measured_linear(&small, &large, &scan);
        eprintln!(
            "{name}: 100KB {a:?}  1.6MB {b:?}  ratio {:.1}",
            b.as_secs_f64() / a.as_secs_f64().max(1e-9)
        );
        assert!(
            linear,
            "{name}: 16x the input cost more than 32x the time, twice: {a:?} -> {b:?}"
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

/// The measurement still catches what it exists to catch: a quadratic scan
/// reads as superlinear, a linear one does not.
#[test]
fn the_measurement_separates_a_quadratic_scan_from_a_linear_one() {
    let (small, large) = ("x".repeat(10_000), "x".repeat(160_000));
    let linear = |s: &str| spin(s.len(), 200);
    let quadratic = |s: &str| spin(s.len() * s.len() / 10_000, 200);
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

/// A burst of load that lasts as long as the large phase of a sequential
/// measurement must not make a linear scan look superlinear.
#[test]
fn a_burst_of_load_over_one_phase_does_not_fail_a_linear_scan() {
    let (small, large) = ("x".repeat(10_000), "x".repeat(160_000));
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
