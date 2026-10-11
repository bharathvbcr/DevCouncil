// Wall-clock budgets for tests, scaled for slow runs.
//
// The model is GitPulse's `coverage_relaxed`: production timeouts stay as they
// are and only tests call this. Coverage instrumentation multiplies the cost of
// every process and thread a deadline waits on, and a loaded or shared CI runner
// does the same, so a fixed sub-second or few-second bound flakes without any
// behaviour having changed. Included with
//
//     include!("../../testsupport/timing.rs");
//
// from a test file (path relative to the including file under
// rust/<crate>/tests or rust/<crate>/src).

/// `DEVMAP_TEST_TIME_SCALE`: a whole multiplier from 1 to 64 for a slow host.
const TIME_SCALE_ENV: &str = "DEVMAP_TEST_TIME_SCALE";

/// The budget after scaling. `scale` multiplies every run; an instrumented run
/// multiplies by 8 and is never below five seconds.
pub(crate) fn relaxed_with(
    duration: std::time::Duration,
    instrumented: bool,
    scale: u32,
) -> std::time::Duration {
    let scaled = duration.saturating_mul(scale.clamp(1, 64));
    if instrumented {
        scaled
            .saturating_mul(8)
            .max(std::time::Duration::from_secs(5))
    } else {
        scaled
    }
}

/// Scale `duration` for this run, from the environment. A malformed
/// `DEVMAP_TEST_TIME_SCALE` panics: a typo that silently left the budget at 1
/// would put the flake back and say nothing.
pub(crate) fn coverage_relaxed(duration: std::time::Duration) -> std::time::Duration {
    let instrumented = std::env::var_os("CARGO_LLVM_COV").is_some()
        || std::env::var_os("LLVM_PROFILE_FILE").is_some();
    let scale = match std::env::var(TIME_SCALE_ENV) {
        Ok(raw) => raw
            .trim()
            .parse::<u32>()
            .unwrap_or_else(|_| panic!("{TIME_SCALE_ENV}={raw:?} is not a whole number")),
        Err(_) => 1,
    };
    relaxed_with(duration, instrumented, scale)
}
