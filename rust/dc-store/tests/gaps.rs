//! What `gap-upsert` stores, the `gaps` reply must return.
//!
//! `gap-upsert` reads `--requirement-id`, `--acceptance-criterion-id`,
//! `--expected-verification-method`, `--file`, `--line` and
//! `--suggested-command`, and the `gaps` table stores all six, but the `gaps`
//! command emitted none of them. A gap read back across the boundary therefore
//! no longer said which criterion it was about, where in the code it was, or
//! what to run to reproduce it — the parts an agent needs in order to act on it.

mod support;
use support::{dcstore, seeded};

#[test]
fn a_gaps_linkage_and_location_reach_the_gaps_reply() {
    let db = seeded("gaps-linkage");
    let upsert = dcstore(
        &db,
        &[
            "gap-upsert",
            "--id",
            "G-AC",
            "--task",
            "TASK-1",
            "--gap-type",
            "acceptance_criteria_unproven",
            "--description",
            "AC-1 has no passing evidence",
            "--requirement-id",
            "REQ-1",
            "--acceptance-criterion-id",
            "AC-1",
            "--expected-verification-method",
            "unit_test",
            "--file",
            "src/a.rs",
            "--line",
            "42",
            "--suggested-command",
            "cargo test -p a",
        ],
    );
    assert_eq!(upsert.code, 0, "gap-upsert failed: {}", upsert.stdout);

    let reply = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert_eq!(reply.code, 0, "gaps failed: {}", reply.stdout);
    for want in [
        "\"requirement_id\":\"REQ-1\"",
        "\"acceptance_criterion_id\":\"AC-1\"",
        "\"expected_verification_method\":\"unit_test\"",
        "\"file\":\"src/a.rs\"",
        "\"line\":42",
        "\"suggested_command\":\"cargo test -p a\"",
    ] {
        assert!(
            reply.stdout.contains(want),
            "{want} missing from the gaps reply: {}",
            reply.stdout
        );
    }
}

/// An unset field answers `null`, not `""` and not a missing key: "this gap is
/// not about a criterion" is a different fact from "it is about a criterion
/// whose id is empty", and a missing key is indistinguishable from an older
/// store that never reported the field.
#[test]
fn an_unlinked_gap_reports_null_linkage_and_location() {
    let db = seeded("gaps-unlinked");
    let upsert = dcstore(
        &db,
        &[
            "gap-upsert",
            "--id",
            "G-STUB",
            "--task",
            "TASK-1",
            "--gap-type",
            "stub_detected",
            "--description",
            "stub",
        ],
    );
    assert_eq!(upsert.code, 0, "gap-upsert failed: {}", upsert.stdout);

    let reply = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert_eq!(reply.code, 0, "gaps failed: {}", reply.stdout);
    for want in [
        "\"requirement_id\":null",
        "\"acceptance_criterion_id\":null",
        "\"expected_verification_method\":null",
        "\"file\":null",
        "\"line\":null",
        "\"suggested_command\":null",
    ] {
        assert!(
            reply.stdout.contains(want),
            "{want} missing from the gaps reply: {}",
            reply.stdout
        );
    }
}

/// `--line` that is not a number is refused, not stored as "no line".
///
/// It used to be parsed with `.ok()`, so `--line 4x` wrote a gap with no
/// location and reported success — the caller believed it had recorded where
/// the gap was.
#[test]
fn a_malformed_line_is_refused_not_dropped() {
    let db = seeded("gaps-bad-line");
    let upsert = dcstore(
        &db,
        &[
            "gap-upsert",
            "--id",
            "G-BAD",
            "--task",
            "TASK-1",
            "--gap-type",
            "stub_detected",
            "--description",
            "stub",
            "--line",
            "4x",
        ],
    );
    assert_ne!(
        upsert.code, 0,
        "a non-numeric --line was accepted: {}",
        upsert.stdout
    );
    assert!(
        upsert.stdout.contains("--line"),
        "the refusal must name the flag: {}",
        upsert.stdout
    );

    let reply = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert!(
        !reply.stdout.contains("G-BAD"),
        "a refused gap was stored anyway: {}",
        reply.stdout
    );
}
