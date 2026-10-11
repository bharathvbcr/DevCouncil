//! A long line must cost time proportional to its length.
//!
//! The ratio, not a wall-clock bound, is what proves linearity: a bound loose
//! enough for a slow CI machine passes a quadratic scan on a short input.
//! Each shape is measured at 100 KB and 400 KB (12.5 and 50 in a debug build), min of several runs, and
//! 4x the input may cost at most ~5x the time (linear is 4x; quadratic is 16x).

use dc_redact::{contains_secret, redact_secrets};
use std::time::{Duration, Instant};

fn best_of(line: &str, runs: usize) -> Duration {
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            let _ = contains_secret(line);
            let _ = redact_secrets(line);
            started.elapsed()
        })
        .min()
        .expect("runs > 0")
}

/// Every assignment- and flag-shaped credential whose literal is judged at each
/// separator of one long line.
#[test]
fn repeated_credential_assignments_scale_linearly() {
    let shapes: [(&str, &str); 14] = [
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
    // 100 KB against 400 KB in a release build. A debug build scans some twenty
    // times slower, so it measures an eighth of that: the ratio is the same.
    let scale = if cfg!(debug_assertions) {
        25_000
    } else {
        100_000
    };
    for (name, unit) in shapes {
        let small = unit.repeat(scale / unit.len());
        let large = unit.repeat(4 * scale / unit.len());
        let (a, b) = (best_of(&small, 9), best_of(&large, 9));
        eprintln!(
            "{name}: 100KB {a:?}  400KB {b:?}  ratio {:.1}",
            b.as_secs_f64() / a.as_secs_f64().max(1e-9)
        );
        // A floor keeps timer noise on a fast scan from reading as a ratio.
        let floor = Duration::from_millis(20);
        assert!(
            b <= floor.max(a * 5),
            "{name}: 4x the input cost more than 5x the time: {a:?} -> {b:?}"
        );
    }
}
