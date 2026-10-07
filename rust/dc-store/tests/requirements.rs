//! The two columns the store creates and never reads.
//!
//! `schema.rs` declares `requirement_ids_json` and
//! `acceptance_criterion_ids_json` because they are in the DevCouncil schema
//! this store was transcribed from, and DevCouncil's planner writes them: they
//! are the link from a task back to the requirement it exists to satisfy and
//! to the acceptance criteria it is accountable for proving.
//!
//! `Store::task` selected neither. Every consumer on this side of the boundary
//! therefore saw a task with no requirements and no acceptance criteria — not
//! *empty*, but absent, which is the more dangerous shape: a requirement
//! coverage gate reading this store would find nothing to check and report a
//! task that satisfies no requirement exactly as it reports one that satisfies
//! all of them. That is the non-cheating invariant broken at the read layer,
//! and it is why this is fixed before anything is built on top of it.
//!
//! The columns are read verbatim and never merged. `planned_files` merges its
//! `agent_appended_*` sibling because an executor may widen its own file scope;
//! there is deliberately no such sibling here. A task may not decide for itself
//! which requirements it satisfies — that is the planner's judgement, and a
//! task that could append to it could discharge a requirement by claiming it.

use dc_store::Store;

mod support;
use support::{dcstore, seeded};

/// Plants a task carrying both links, exactly as DevCouncil's planner writes
/// them, and returns the database path.
fn task_with_links(name: &str) -> std::path::PathBuf {
    let db = seeded(name);
    let store = Store::open(&db).expect("open store");
    store
        .connection()
        .execute(
            "INSERT INTO tasks (id, title, description, status, \
             requirement_ids_json, acceptance_criterion_ids_json) \
             VALUES ('TASK-1', 'planted', '', 'ready', \
             '[\"REQ-1\",\"REQ-2\"]', '[\"AC-1\",\"AC-2\",\"AC-3\"]')",
            [],
        )
        .expect("plant task");
    db
}

#[test]
fn a_tasks_requirements_survive_the_read() {
    let db = task_with_links("requirements-read");
    let store = Store::open(&db).expect("open store");
    let task = store
        .task("TASK-1")
        .expect("read task")
        .expect("task exists");

    assert_eq!(
        task.requirement_ids_json, "[\"REQ-1\",\"REQ-2\"]",
        "the store dropped the requirements the planner linked; a coverage gate \
         reading this would see a task accountable to nothing"
    );
    assert_eq!(
        task.acceptance_criterion_ids_json, "[\"AC-1\",\"AC-2\",\"AC-3\"]",
        "the store dropped the acceptance criteria the task is meant to prove"
    );
}

#[test]
fn the_links_reach_the_boundary_reply() {
    let db = task_with_links("requirements-boundary");
    let reply = dcstore(&db, &["task", "--task", "TASK-1"]);

    assert_eq!(reply.code, 0, "task read failed: {}", reply.stdout);
    assert!(
        reply
            .stdout
            .contains("\"requirement_ids\":[\"REQ-1\",\"REQ-2\"]"),
        "requirement_ids never reached the Go plane: {}",
        reply.stdout
    );
    assert!(
        reply
            .stdout
            .contains("\"acceptance_criterion_ids\":[\"AC-1\",\"AC-2\",\"AC-3\"]"),
        "acceptance_criterion_ids never reached the Go plane: {}",
        reply.stdout
    );
}

/// A task with no links reads as an empty array, not as a missing key.
///
/// The distinction is the whole point of the fix. A consumer that gets no key
/// cannot tell "this task satisfies no requirement" from "this store does not
/// report requirements", and the schema's own `DEFAULT '[]'` says the first of
/// those is a real, representable state.
#[test]
fn a_task_with_no_links_reads_as_empty_rather_than_absent() {
    let db = seeded("requirements-empty");
    let store = Store::open(&db).expect("open store");
    store
        .connection()
        .execute(
            "INSERT INTO tasks (id, title, description, status) \
             VALUES ('TASK-1', 'planted', '', 'ready')",
            [],
        )
        .expect("plant task");
    drop(store);

    let store = Store::open(&db).expect("reopen store");
    let task = store
        .task("TASK-1")
        .expect("read task")
        .expect("task exists");
    assert_eq!(task.requirement_ids_json, "[]");
    assert_eq!(task.acceptance_criterion_ids_json, "[]");

    let reply = dcstore(&db, &["task", "--task", "TASK-1"]);
    assert!(
        reply.stdout.contains("\"requirement_ids\":[]"),
        "an unlinked task must report an empty list, not omit the key: {}",
        reply.stdout
    );
}

/// Plants two requirements and a task linking them plus one id no row defines.
fn task_with_requirement_rows(name: &str) -> std::path::PathBuf {
    let db = task_with_links(name);
    let store = Store::open(&db).expect("open store");
    store
        .connection()
        .execute_batch(
            "UPDATE tasks SET requirement_ids_json = '[\"REQ-2\",\"REQ-GONE\",\"REQ-1\"]' \
               WHERE id = 'TASK-1'; \
             INSERT INTO requirements (id, title, description, priority, source, \
               acceptance_criteria_json) VALUES \
               ('REQ-1', 'sums', 'adds two numbers', 'high', 'user', \
                '[{\"id\":\"AC-1\",\"description\":\"adds\",\"verification_method\":\"unit_test\"}]'), \
               ('REQ-2', 'errors', 'says \"why\"', 'low', 'planner', '[]'), \
               ('REQ-UNLINKED', 'other', '', 'low', 'planner', '[]');",
        )
        .expect("plant requirements");
    db
}

/// The verifier dispatches on each criterion's verification method, so it has
/// to be able to read the requirement rows a task links to. Before this the
/// table was created and read by nothing, and every method was validated and
/// then ignored.
#[test]
fn a_tasks_linked_requirements_are_read_in_link_order() {
    let db = task_with_requirement_rows("requirements-rows");
    let store = Store::open(&db).expect("open store");
    let linked = store
        .task_requirements("TASK-1")
        .expect("read requirements")
        .expect("task exists");

    let ids: Vec<&str> = linked.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        ["REQ-2", "REQ-1"],
        "rows in the order the task links them"
    );
    assert_eq!(
        linked.missing,
        ["REQ-GONE"],
        "a linked id with no row must be reported, not dropped: a criterion \
         nobody can read is not a criterion that passed"
    );
    assert!(!linked.truncated);
    assert_eq!(linked.rows[1].priority, "high");
    assert!(
        linked.rows[1]
            .acceptance_criteria_json
            .contains("unit_test")
    );
}

#[test]
fn an_unknown_task_has_no_requirements_answer() {
    let db = seeded("requirements-unknown-task");
    let store = Store::open(&db).expect("open store");
    assert!(
        store
            .task_requirements("NOPE")
            .expect("read requirements")
            .is_none()
    );
}

#[test]
fn the_requirements_reach_the_boundary_reply() {
    let db = task_with_requirement_rows("requirements-reply");
    let reply = dcstore(&db, &["requirements", "--task", "TASK-1"]);

    assert_eq!(reply.code, 0, "requirements read failed: {}", reply.stdout);
    // The criteria column is carried as a JSON string, so a malformed row
    // reaches the Go plane as a decode error it reports rather than as a
    // reply that no longer parses.
    assert!(
        reply
            .stdout
            .contains("\"acceptance_criteria_json\":\"[{\\\"id\\\":\\\"AC-1\\\""),
        "criteria column not carried verbatim: {}",
        reply.stdout
    );
    assert!(
        reply.stdout.contains("\"missing\":[\"REQ-GONE\"]"),
        "missing ids not reported: {}",
        reply.stdout
    );
    assert!(
        !reply.stdout.contains("REQ-UNLINKED"),
        "a requirement the task does not link leaked into its answer: {}",
        reply.stdout
    );
    let unknown = dcstore(&db, &["requirements", "--task", "NOPE"]);
    assert_eq!(unknown.code, 0, "{}", unknown.stdout);
    assert!(
        unknown.stdout.contains("\"task_found\":false"),
        "an unknown task must say so: {}",
        unknown.stdout
    );
}

/// The same fail-closed rule the other scope columns get.
///
/// These values are embedded into the reply as raw JSON text, so a column that
/// is not a well-formed array is either a broken document or an injection. The
/// store shares its file with DevCouncil and with anything else holding the
/// path, so the read asserts it rather than trusting every writer.
#[test]
fn a_malformed_requirement_column_is_an_error_not_a_broken_reply() {
    let db = seeded("requirements-malformed");
    let store = Store::open(&db).expect("open store");
    store
        .connection()
        .execute(
            "INSERT INTO tasks (id, title, description, status, requirement_ids_json) \
             VALUES ('TASK-1', 'planted', '', 'ready', '\"REQ-1\"')",
            [],
        )
        .expect("plant task");
    drop(store);

    let store = Store::open(&db).expect("reopen store");
    let err = store
        .task("TASK-1")
        .expect_err("a non-array requirement column must not read as a task");
    let message = err.to_string();
    assert!(
        message.contains("requirement_ids_json"),
        "the error must name the column that is wrong: {message}"
    );
}
