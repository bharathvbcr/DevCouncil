//! `WriteBreakdown`'s arithmetic, in a build without grammars.
//!
//! Split from `write_breakdown.rs`, whose other cases measure real generation
//! writes built by extracting source and so are declared
//! `required-features = ["parse"]`. The type is public and ungated so that an
//! embedder reading a persisted map can name it; this is the half of its
//! contract that needs no write to check.

use devmap_store::WriteBreakdown;

/// The residual is a residual: it is whatever the named spans did not cover.
///
/// Asserted against a breakdown built by hand rather than by a write, because
/// the property is arithmetic and a real write cannot be made to have a chosen
/// set of span values. The write-path half of this is
/// `the_split_accounts_for_the_write_without_exceeding_it` in
/// `write_breakdown.rs`.
#[test]
fn the_residual_is_the_total_minus_every_named_span() {
    let mut breakdown = WriteBreakdown {
        unresolved: 0.25,
        commit: 0.05,
        ..Default::default()
    };
    breakdown.attribute_residual(1.0);
    assert!(
        (breakdown.other - 0.70).abs() < 1e-9,
        "residual of a 1.0s write charging 0.30s should be 0.70s, got {}",
        breakdown.other
    );
    let charged: f64 = breakdown.parts().iter().map(|(_, secs)| secs).sum();
    assert!(
        (charged - 1.0).abs() < 1e-9,
        "parts() must sum to the total once the residual is attributed, got {charged}"
    );

    // A total below what the spans already charged is a clock that went
    // backwards, not a negative phase. Reporting a negative duration would put
    // a `-0.5s` span in front of a reader and into the `--json` timings.
    let mut backwards = WriteBreakdown {
        unresolved: 2.0,
        ..Default::default()
    };
    backwards.attribute_residual(1.0);
    assert_eq!(
        backwards.other, 0.0,
        "a residual can never be negative, however the clock behaves"
    );
}
