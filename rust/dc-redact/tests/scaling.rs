//! A long line must cost time proportional to its length.
//!
//! The ratio, not a wall-clock bound, is what proves linearity: a bound loose
//! enough for a slow CI machine passes a quadratic scan on a short input.
//!
//! Each shape is measured on a small input and on one `GROWTH` (4) times
//! longer. Linear scanning makes the large scan cost 4x the small one;
//! quadratic scanning 16x. The large scan may cost at most `MAX_RATIO` (10)
//! times the small one.
//!
//! Wall-clock time under contention is only ever too slow, never too fast, and
//! a scan that finds one quiet window is as fast as it can be. So the two are
//! timed in alternation, the minimum of each is kept, and sampling continues
//! (up to `MAX_RUNS`) until the minima pass. Passing needs one quiet window per
//! side, which more runs only make likelier; a quadratic scan cannot pass at
//! all, because no run of it is faster than its own cost. A quiet machine
//! passes after `MIN_RUNS`.
//!
//! Both inputs are sized so one small scan takes about `SMALL_SCAN` (5 ms) and
//! one large scan about 20 ms: short enough to fit in a scheduler timeslice, so
//! a quiet window exists for the large side too. Longer scans were tried and
//! cannot find one on a saturated CPU: 18 `yes` burners on 18 cores inflated
//! a 100-400 ms scan by up to 3x while a 5-15 ms scan ran undisturbed, so a
//! linear scan read 29x for 4x the input.
//!
//! Margins. The unmodified scan was measured at 1x, 2x, 4x, 8x and 16x
//! (release, min of 15): every doubling costs 1.9x to 2.2x, so 4x costs 3.6x
//! to 4.8x. A CI runner measured 5.13x (400 KB of input and as much again of
//! redacted output stop fitting where 100 KB did). The bound of 10 is 1.95x
//! above that reading and 1.6x below a quadratic scan's 16.
//!
//! What this cannot catch: a constant-factor slowdown, because both sides slow
//! alike, and growth below the allowance. Over a 4x range a scan costing n^1.5
//! reads 8x and passes; the first exponent that fails is about n^1.66.

use dc_redact::{contains_secret, redact_secrets};
use std::time::{Duration, Instant};

/// How many times the large input is longer than the small one.
const GROWTH: usize = 4;

/// The most the large scan may cost, as a multiple of the small one. Linear is
/// `GROWTH`, quadratic is `GROWTH * GROWTH`.
const MAX_RATIO: u32 = 10;

/// What one small scan should take.
const SMALL_SCAN: Duration = Duration::from_millis(5);

/// The fewest alternating runs before the minima are judged, and the most
/// before they are given up on.
const MIN_RUNS: usize = 5;
const MAX_RUNS: usize = 60;

/// A shape whose small scan costs this much even at the floor size (two do in
/// a debug build, at 100 us a byte) spans many timeslices on its own. It gets
/// `HEAVY_RUNS` runs, to keep the whole test near 30 s.
const HEAVY_SMALL_SCAN: Duration = Duration::from_millis(100);
const HEAVY_RUNS: usize = 3;

/// The smallest small input, in bytes. The cost per byte is still rising below
/// it: a debug build measured 2.2 us a byte at 4 KB and 2.9 at 8 KB on the
/// netrc shape, flat from there, and 65 us at 1 KB against 97 at 2 KB on
/// another. A smaller small input would read that step as superlinear growth
/// (5-8x for 4x) and leave the bound with no margin.
const MIN_SMALL_BYTES: usize = 8192;

/// The largest small input calibration will build, in bytes.
const MAX_SMALL_BYTES: usize = 1 << 20;

/// The cheapest timings of `work` on `small` and on `large`, taken in
/// alternation: small, large, small, large. Stops after `MIN_RUNS` once the
/// minima are within the bound, or after `max_runs`.
///
/// The two inputs used to be timed one after the other, nine runs of one and
/// then nine of the other. A burst of load that covered the second phase — a
/// loaded CI host, other test binaries — then inflated only the large input's
/// time, and the ratio read 7x for a scan that is linear. Alternating means
/// any burst reaches both inputs, and the minimum of each keeps the runs the
/// burst missed. It is still wall-clock time: the scan is one thread that does
/// not wait, so the quiet runs are its CPU cost.
fn best_pair(
    small: &str,
    large: &str,
    min_runs: usize,
    max_runs: usize,
    work: &dyn Fn(&str),
) -> (Duration, Duration) {
    let time = |input: &str| {
        let started = Instant::now();
        work(input);
        started.elapsed()
    };
    let (mut a, mut b) = (Duration::MAX, Duration::MAX);
    for run in 1..=max_runs {
        a = a.min(time(small));
        b = b.min(time(large));
        if run >= min_runs && is_linear(a, b) {
            break;
        }
    }
    (a, b)
}

fn scan(line: &str) {
    let _ = contains_secret(line);
    let _ = redact_secrets(line);
}

/// The large scan may cost at most `MAX_RATIO` times the small one.
fn is_linear(small: Duration, large: Duration) -> bool {
    large <= small * MAX_RATIO
}

/// The repetitions of `unit` whose small scan takes `SMALL_SCAN` (at least
/// `MIN_SMALL_BYTES`), and that scan's cost.
///
/// Found by doubling from `MIN_SMALL_BYTES` until the cheapest of three runs
/// reaches `SMALL_SCAN`, then sized from the cheapest rate (nanoseconds a byte)
/// seen at any size: contention can only make a run read slow, so the doubling
/// may stop on a size whose quiet cost is under the target, and the cheapest
/// rate is the closest to the scan's own. Capped at `MAX_SMALL_BYTES`.
fn calibrate(unit: &str, work: &dyn Fn(&str)) -> (usize, Duration) {
    let mut reps = MIN_SMALL_BYTES.div_ceil(unit.len());
    let mut best_nanos_per_byte = f64::MAX;
    loop {
        let input = unit.repeat(reps);
        let cheapest = (0..3)
            .map(|_| {
                let started = Instant::now();
                work(&input);
                started.elapsed()
            })
            .min()
            .unwrap_or(Duration::ZERO);
        best_nanos_per_byte =
            best_nanos_per_byte.min(cheapest.as_nanos() as f64 / input.len() as f64);
        if cheapest >= SMALL_SCAN || input.len() * 2 > MAX_SMALL_BYTES {
            break;
        }
        reps *= 2;
    }
    let wanted_bytes = SMALL_SCAN.as_nanos() as f64 / best_nanos_per_byte;
    let reps = ((wanted_bytes / unit.len() as f64).ceil() as usize).clamp(
        MIN_SMALL_BYTES.div_ceil(unit.len()),
        MAX_SMALL_BYTES / unit.len(),
    );
    let cost = best_nanos_per_byte * (reps * unit.len()) as f64;
    (reps, Duration::from_nanos(cost as u64))
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
    for (name, unit) in shapes {
        let (reps, cost) = calibrate(unit, &scan);
        let small = unit.repeat(reps);
        let large = unit.repeat(GROWTH * reps);
        // A shape whose floor-sized input already takes 100 ms or more (two
        // do in a debug build) spans many timeslices on its own; it gets few
        // runs so the whole test stays near 30 s.
        let (min_runs, max_runs) = if cost >= HEAVY_SMALL_SCAN {
            (HEAVY_RUNS, HEAVY_RUNS + 1)
        } else {
            (MIN_RUNS, MAX_RUNS)
        };
        let (a, b) = best_pair(&small, &large, min_runs, max_runs, &scan);
        eprintln!(
            "{name}: {}B {a:?}  {}B {b:?}  ratio {:.2} (linear is {GROWTH}, bound {MAX_RATIO})",
            small.len(),
            large.len(),
            b.as_secs_f64() / a.as_secs_f64().max(1e-9)
        );
        assert!(
            is_linear(a, b),
            "{name}: {GROWTH}x the input cost more than {MAX_RATIO}x the time: {a:?} -> {b:?}"
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

/// A linear synthetic scan of `SYNTHETIC_SMALL` bytes at `SYNTHETIC_NANOS` a
/// byte takes `SMALL_SCAN`.
const SYNTHETIC_SMALL: usize = 25_000;
const SYNTHETIC_NANOS: u64 = 200;

/// Runs a failing synthetic scan is given: enough to show it never passes.
const SYNTHETIC_MAX_RUNS: usize = 12;

fn synthetic_inputs() -> (String, String) {
    (
        "x".repeat(SYNTHETIC_SMALL),
        "x".repeat(GROWTH * SYNTHETIC_SMALL),
    )
}

/// The measurement still catches what it exists to catch: a quadratic scan
/// reads as superlinear, a linear one does not.
#[test]
fn the_measurement_separates_a_quadratic_scan_from_a_linear_one() {
    let (small, large) = synthetic_inputs();
    let linear = |s: &str| spin(s.len(), SYNTHETIC_NANOS);
    let quadratic = |s: &str| spin(s.len() * s.len() / SYNTHETIC_SMALL, SYNTHETIC_NANOS);
    let (a, b) = best_pair(&small, &large, MIN_RUNS, SYNTHETIC_MAX_RUNS, &linear);
    assert!(
        is_linear(a, b),
        "a linear scan was called superlinear: {a:?} -> {b:?}"
    );
    let (a, b) = best_pair(&small, &large, MIN_RUNS, SYNTHETIC_MAX_RUNS, &quadratic);
    assert!(
        !is_linear(a, b),
        "a quadratic scan passed as linear: {a:?} -> {b:?}"
    );
}

/// A scan whose cost per byte grows by the 25% a cache boundary adds, the
/// shape that failed on macos-latest at 5.0x for 4x, is still linear; one that
/// grows as n^1.8 (a ratio of 12) is not.
#[test]
fn a_cache_sized_step_in_cost_per_byte_is_linear_and_a_slow_climb_is_not() {
    let (small, large) = synthetic_inputs();
    let cache_step = |s: &str| {
        let per_byte = if s.len() > small.len() {
            SYNTHETIC_NANOS * 5 / 4
        } else {
            SYNTHETIC_NANOS
        };
        spin(s.len(), per_byte);
    };
    let (a, b) = best_pair(&small, &large, MIN_RUNS, SYNTHETIC_MAX_RUNS, &cache_step);
    assert!(
        is_linear(a, b),
        "a 25% step in cost per byte was called superlinear: {a:?} -> {b:?}"
    );
    let n_to_1_8 = |s: &str| {
        let units = (s.len() as f64).powf(1.8) / (SYNTHETIC_SMALL as f64).powf(0.8);
        spin(units as usize, SYNTHETIC_NANOS);
    };
    let (a, b) = best_pair(&small, &large, MIN_RUNS, SYNTHETIC_MAX_RUNS, &n_to_1_8);
    assert!(
        !is_linear(a, b),
        "an n^1.8 scan passed as linear: {a:?} -> {b:?}"
    );
}

/// A burst of load that covers the start of the measurement, longer than
/// `MIN_RUNS` runs, must not make a linear scan look superlinear: sampling goes
/// on until a quiet run lands.
#[test]
fn a_burst_of_load_over_the_first_runs_does_not_fail_a_linear_scan() {
    let (small, large) = synthetic_inputs();
    let started = Instant::now();
    // Each run is about 25 ms in the clear, 150 ms in the burst: the burst
    // covers the first `MIN_RUNS` and more.
    let noisy = |s: &str| {
        let burst = if started.elapsed() < Duration::from_millis(600) {
            6
        } else {
            1
        };
        spin(s.len(), SYNTHETIC_NANOS * burst);
    };
    let (a, b) = best_pair(&small, &large, MIN_RUNS, MAX_RUNS, &noisy);
    assert!(
        is_linear(a, b),
        "a load burst read as superlinear: {a:?} -> {b:?}"
    );
}
