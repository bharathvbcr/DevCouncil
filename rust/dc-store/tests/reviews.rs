//! `review-append` and `reviews` through the binary: the seam the Go host
//! records and reads `llm_review` evidence across.
//!
//! A review is the only evidence that discharges an `llm_review` criterion, so
//! the boundary is held to the same rules as every other writer here: a value
//! that would let the reply stop parsing is refused on the way in, a row that
//! no longer parses is carried as null beside an error on the way out, and an
//! answer never shares a key with a different answer's type.

use dc_store::json::check_object;
use rusqlite::Connection;

mod support;
use support::{dcstore, plant_db, seeded};

const SUBJECT: &str = "git-tree:4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const RECORD: &str = r#"{"schema":1,"reviewers":[{"id":"a"},{"id":"b"}]}"#;

fn append(db: &std::path::Path, task: &str, criterion: &str, record: &str) -> support::Reply {
    dcstore(
        db,
        &[
            "review-append",
            "--task",
            task,
            "--acceptance-criterion-id",
            criterion,
            "--subject-hash",
            SUBJECT,
            "--record",
            record,
        ],
    )
}

#[test]
fn a_review_round_trips_with_its_record_embedded_as_an_object() {
    let db = seeded("reviews-roundtrip");
    plant_db(&db, "TASK-1", "ready", "{}");

    let reply = append(&db, "TASK-1", "AC-1", RECORD);
    assert_eq!(reply.code, 0, "{}", reply.stdout);
    assert!(reply.stdout.contains("\"review_id\":1"), "{}", reply.stdout);
    assert!(
        !reply.stdout.contains("\"id\""),
        "the answer must not share `id` with evidence-append: {}",
        reply.stdout
    );
    assert_eq!(append(&db, "TASK-1", "AC-2", RECORD).code, 0);

    let listed = dcstore(
        &db,
        &[
            "reviews",
            "--task",
            "TASK-1",
            "--acceptance-criterion-id",
            "AC-1",
        ],
    );
    assert_eq!(listed.code, 0, "{}", listed.stdout);
    check_object(&listed.stdout).unwrap_or_else(|why| panic!("{why}: {}", listed.stdout));
    assert!(
        listed.stdout.contains(&format!("\"record\":{RECORD}")),
        "the record must arrive as the object it was, not a string: {}",
        listed.stdout
    );
    assert!(
        listed
            .stdout
            .contains(&format!("\"subject_hash\":\"{SUBJECT}\""))
    );
    assert!(listed.stdout.contains("\"shown\":1"), "{}", listed.stdout);
    assert!(!listed.stdout.contains("AC-2"), "{}", listed.stdout);

    let all = dcstore(&db, &["reviews", "--task", "TASK-1"]);
    assert!(all.stdout.contains("\"shown\":2"), "{}", all.stdout);
}

#[test]
fn refusals_leave_no_row_and_say_why() {
    let db = seeded("reviews-refused");
    plant_db(&db, "TASK-1", "ready", "{}");
    for (task, record, why) in [
        ("TASK-404", RECORD, "no task"),
        ("TASK-1", "[]", "JSON object"),
        ("TASK-1", r#"{"a":1}}"#, "well-formed"),
        ("TASK-1", r#"{"a":1},"x":{"#, "well-formed"),
    ] {
        let reply = append(&db, task, "AC-1", record);
        assert_ne!(reply.code, 0, "{record:?} for {task} was stored");
        assert!(reply.stdout.contains(why), "{why}: {}", reply.stdout);
    }
    let missing = dcstore(
        &db,
        &[
            "review-append",
            "--task",
            "TASK-1",
            "--acceptance-criterion-id",
            "AC-1",
        ],
    );
    assert_ne!(missing.code, 0);
    assert!(
        missing.stdout.contains("--subject-hash is required"),
        "{}",
        missing.stdout
    );
    let listed = dcstore(&db, &["reviews", "--task", "TASK-1"]);
    assert!(
        listed.stdout.contains("\"reviews\":[]"),
        "{}",
        listed.stdout
    );
    assert_ne!(
        dcstore(&db, &["reviews"]).code,
        0,
        "a list with no task was answered"
    );
}

#[test]
fn a_row_that_no_longer_parses_is_carried_as_null_beside_an_error() {
    let db = seeded("reviews-corrupt");
    plant_db(&db, "TASK-1", "ready", "{}");
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO reviews (task_id, acceptance_criterion_id, subject_hash, record_json, recorded_at)
             VALUES ('TASK-1', 'AC-1', 'git-tree:00', '{\"x\":[1}', '2026-10-25T00:00:00Z')",
            [],
        )
        .unwrap();
    let reply = dcstore(&db, &["reviews", "--task", "TASK-1"]);
    assert_eq!(reply.code, 0, "{}", reply.stdout);
    check_object(&reply.stdout).unwrap_or_else(|why| panic!("{why}: {}", reply.stdout));
    assert!(reply.stdout.contains("\"record\":null"), "{}", reply.stdout);
    assert!(
        reply.stdout.contains("\"record_error\""),
        "{}",
        reply.stdout
    );
}

#[test]
fn health_reports_the_schema_that_carries_reviews() {
    let db = seeded("reviews-health");
    let reply = dcstore(&db, &["health"]);
    assert!(
        reply
            .stdout
            .contains(&format!("\"schema_version\":{}", dc_store::SCHEMA_VERSION)),
        "{}",
        reply.stdout
    );
    let version: i64 = Connection::open(&db)
        .unwrap()
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 11);
}
