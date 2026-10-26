//! What `gap-upsert` stores, the `gaps` reply must return.
//!
//! `gap-upsert` read `--requirement-id`, `--acceptance-criterion-id`,
//! `--expected-verification-method`, `--file`, `--line` and
//! `--suggested-command`, and the `gaps` table stored all six, but the `gaps`
//! command emitted none of them. A gap read back across the boundary therefore
//! no longer said which criterion it was about, where in the code it was, or
//! what to run to reproduce it — the parts an agent needs in order to act on it.
//!
//! The paths to a failed command's captured output had no column at all, so a
//! command gap could only be read with its logs from the verify call that
//! raised it. `--stdout-path` and `--stderr-path` close that.

use dc_store::Store;
use rusqlite::Connection;

mod support;
use support::{dcstore, seeded, temp_dir};

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
            "--stdout-path",
            ".devcouncil/runs/r1/stdout.log",
            "--stderr-path",
            ".devcouncil/runs/r1/stderr.log",
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
        "\"stdout_path\":\".devcouncil/runs/r1/stdout.log\"",
        "\"stderr_path\":\".devcouncil/runs/r1/stderr.log\"",
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
        "\"stdout_path\":null",
        "\"stderr_path\":null",
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

// A store created before `gaps` had output-path columns gains them on open.
//
// Every statement in the schema is `CREATE … IF NOT EXISTS`, so a column added
// to `CREATE TABLE gaps` reaches only stores created after the change. An
// existing store keeps its old table, and the first `gap-upsert` naming
// `stdout_path` would fail with "no such column" — on exactly the stores that
// have been in use longest. Every other test here runs on a fresh store and
// cannot see this, which is why the old table is planted by hand.

/// `gaps` exactly as schema 9 created it before the output-path columns.
const GAPS_BEFORE_OUTPUT_PATHS: &str = "
CREATE TABLE gaps (
    id VARCHAR NOT NULL,
    severity VARCHAR NOT NULL,
    gap_type VARCHAR NOT NULL,
    requirement_id VARCHAR,
    task_id VARCHAR,
    description VARCHAR NOT NULL,
    evidence_json VARCHAR NOT NULL DEFAULT '[]',
    recommended_fix VARCHAR NOT NULL,
    blocking BOOLEAN NOT NULL,
    file VARCHAR,
    line INTEGER,
    suggested_command VARCHAR,
    acceptance_criterion_id VARCHAR,
    expected_verification_method VARCHAR,
    PRIMARY KEY (id)
);
INSERT INTO gaps (id, severity, gap_type, task_id, description, recommended_fix, blocking)
VALUES ('G-OLD', 'low', 'stub_detected', 'TASK-1', 'from before', 'fix', 0);
";

fn gaps_columns(db: &std::path::Path) -> Vec<String> {
    let conn = Connection::open(db).expect("open raw");
    let mut stmt = conn.prepare("PRAGMA table_info(gaps)").expect("table_info");
    stmt.query_map([], |row| row.get::<_, String>("name"))
        .expect("table_info rows")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("table_info names")
}

#[test]
fn an_old_gaps_table_gains_the_output_path_columns_on_open() {
    let db = temp_dir("gaps-upgrade").join("state.sqlite");
    Connection::open(&db)
        .expect("create old store")
        .execute_batch(GAPS_BEFORE_OUTPUT_PATHS)
        .expect("plant the old gaps table");
    assert!(
        !gaps_columns(&db).iter().any(|c| c == "stdout_path"),
        "the planted table must be the old shape or this test proves nothing"
    );

    drop(Store::open(&db).expect("open upgrades the store"));
    let columns = gaps_columns(&db);
    for want in ["stdout_path", "stderr_path"] {
        assert!(
            columns.iter().any(|c| c == want),
            "{want} not added to an existing gaps table: {columns:?}"
        );
    }

    // A second open finds them present and must not try to add them again.
    drop(Store::open(&db).expect("reopening an upgraded store"));
    assert_eq!(gaps_columns(&db), columns, "a reopen changed the table");

    let upsert = dcstore(
        &db,
        &[
            "gap-upsert",
            "--id",
            "G-NEW",
            "--task",
            "TASK-1",
            "--gap-type",
            "test_failure",
            "--description",
            "command failed",
            "--stdout-path",
            "out.log",
            "--stderr-path",
            "err.log",
        ],
    );
    assert_eq!(
        upsert.code, 0,
        "gap-upsert on an upgraded store: {}",
        upsert.stdout
    );

    let reply = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert_eq!(reply.code, 0, "gaps on an upgraded store: {}", reply.stdout);
    assert!(
        reply.stdout.contains("\"stdout_path\":\"out.log\"")
            && reply.stdout.contains("\"stderr_path\":\"err.log\""),
        "the new gap's output paths did not round-trip: {}",
        reply.stdout
    );
    // The row written before the upgrade survives it, with no paths.
    assert!(
        reply.stdout.contains("\"id\":\"G-OLD\""),
        "the upgrade lost a pre-existing gap: {}",
        reply.stdout
    );
}
