//! The wall-clock scaling helper that the load-sensitive tests share.

use std::time::Duration;

include!("../../testsupport/timing.rs");

#[test]
fn a_plain_run_keeps_its_budget() {
    let budget = Duration::from_secs(2);
    assert_eq!(relaxed_with(budget, false, 1), budget);
}

#[test]
fn a_scale_multiplies_and_is_bounded() {
    let budget = Duration::from_secs(2);
    assert_eq!(relaxed_with(budget, false, 4), Duration::from_secs(8));
    assert_eq!(
        relaxed_with(budget, false, 0),
        budget,
        "zero is not a way to remove a deadline"
    );
    assert_eq!(
        relaxed_with(budget, false, u32::MAX),
        Duration::from_secs(128)
    );
}

#[test]
fn an_instrumented_run_is_at_least_five_seconds_and_eight_times_longer() {
    assert_eq!(
        relaxed_with(Duration::from_millis(100), true, 1),
        Duration::from_secs(5)
    );
    assert_eq!(
        relaxed_with(Duration::from_secs(2), true, 1),
        Duration::from_secs(16)
    );
}

#[test]
fn the_environment_is_read_and_a_typo_is_loud() {
    // One test owns the variable; the others call the pure function.
    std::env::set_var(TIME_SCALE_ENV, "3");
    assert!(coverage_relaxed(Duration::from_secs(1)) >= Duration::from_secs(3));
    std::env::set_var(TIME_SCALE_ENV, "three");
    let refused = std::panic::catch_unwind(|| coverage_relaxed(Duration::from_secs(1)));
    std::env::remove_var(TIME_SCALE_ENV);
    assert!(refused.is_err(), "a malformed scale must not read as 1");
}
