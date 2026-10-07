//! A gap's criterion linkage must survive the `gaps` reply.
//!
//! `gap-upsert` reads `--requirement-id`, `--acceptance-criterion-id` and
//! `--expected-verification-method`, and the `gaps` table stores all three, but
//! the `gaps` command emitted none of them. A criterion gap read back across
//! the boundary therefore no longer said which requirement or criterion it was
//! about, or how that criterion was meant to be proven — the part of the gap an
//! agent needs in order to act on it.

mod support;
use support::{dcstore, seeded};

#[test]
fn a_criterion_gaps_linkage_reaches_the_gaps_reply() {
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
        ],
    );
    assert_eq!(upsert.code, 0, "gap-upsert failed: {}", upsert.stdout);

    let reply = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert_eq!(reply.code, 0, "gaps failed: {}", reply.stdout);
    for want in [
        "\"requirement_id\":\"REQ-1\"",
        "\"acceptance_criterion_id\":\"AC-1\"",
        "\"expected_verification_method\":\"unit_test\"",
    ] {
        assert!(
            reply.stdout.contains(want),
            "{want} missing from the gaps reply: {}",
            reply.stdout
        );
    }
}

/// An unlinked gap answers `null`, not `""` and not a missing key: "this gap is
/// not about a criterion" is a different fact from "it is about a criterion
/// whose id is empty", and a missing key is indistinguishable from an older
/// store that never reported the field.
#[test]
fn an_unlinked_gap_reports_null_linkage() {
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
    ] {
        assert!(
            reply.stdout.contains(want),
            "{want} missing from the gaps reply: {}",
            reply.stdout
        );
    }
}
