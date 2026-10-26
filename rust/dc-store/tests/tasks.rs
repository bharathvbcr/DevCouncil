//! Workbench items as the one task source (schema 10 / workbench 14).
//!
//! `scope` is validated where it enters, `ready`/`task`/`acquire` read
//! `work_items`, `blocked` is derived from gaps, and the legacy `tasks` table is
//! archived into items by a migration that can be repeated and interrupted.

use dc_store::{AcquireRequest, ReadyMode, SCHEMA_VERSION, Store, StoreError, schema};
use rusqlite::Connection;
use std::path::Path;

mod support;
use support::{LEGACY_TASKS, dcstore, item_request, plant, plant_db, seeded, temp_dir};

fn put(store: &Store, id: &str, scope: &str) -> Result<String, dc_store::workbench::Error> {
    store.ensure_local_repository().unwrap();
    store.workbench_request("items.put", &item_request(id, "ready", scope))
}

fn refused(store: &Store, id: &str, scope: &str) -> String {
    match put(store, id, scope) {
        Ok(ok) => panic!("scope {scope} was accepted: {ok}"),
        Err(e) => {
            assert_eq!(e.code, "invalid_scope", "{scope}: {}", e.message);
            e.message
        }
    }
}

fn ids(store: &Store, mode: ReadyMode) -> Vec<String> {
    store
        .ready_tasks(mode)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}

fn gap(store: &Store, id: &str, task: &str, blocking: bool) {
    store
        .connection()
        .execute(
            "INSERT INTO gaps (id, severity, gap_type, description, recommended_fix, blocking, task_id)
             VALUES (?1, 'high', 'unproven', 'd', 'fix', ?2, ?3)",
            rusqlite::params![id, blocking, task],
        )
        .unwrap();
}

fn acquire(store: &Store, task: &str, owner: &str) -> Result<dc_store::Lease, StoreError> {
    store.acquire(&AcquireRequest {
        task_id: task.into(),
        owner: owner.into(),
        ttl_seconds: Some(300),
        ..Default::default()
    })
}

// --- scope validation ---------------------------------------------------------

#[test]
fn a_valid_scope_is_stored_and_read_back_as_the_gate_needs_it() {
    let store = Store::open_in_memory().unwrap();
    put(
        &store,
        "S1",
        r#"{"planned_files":["src/a.rs",{"path":"docs/ü/é.md","allowed_change":"create","reason":"new"},{"path":"x/y","allowed_change":"read_only"}],
            "forbidden_changes":["**/.env"],"allowed_commands":["cargo test"],
            "expected_tests":["cargo test -p dc-store"],"requirement_ids":["REQ-1"],
            "acceptance_criterion_ids":["AC-1"],"difficulty":"hard"}"#,
    )
    .unwrap();
    let task = store.task("S1").unwrap().expect("a planted item reads");
    assert_eq!(task.status, "ready");
    assert_eq!(task.difficulty.as_deref(), Some("hard"));
    assert!(task.planned_files_json.contains("docs/ü/é.md"));
    assert_eq!(task.forbidden_changes_json, r#"["**/.env"]"#);
    assert_eq!(task.requirement_ids_json, r#"["REQ-1"]"#);
}

#[test]
fn an_invalid_scope_is_refused_with_a_typed_error_and_nothing_is_stored() {
    let store = Store::open_in_memory().unwrap();
    let cases = [
        r#"{"planned_files":[{"path":"a","allowed_change":"read-only"}]}"#,
        r#"{"planned_files":[{"path":"a","allowed_change":"READ_ONLY"}]}"#,
        r#"{"planned_files":[{"path":"a","allowed_change":"readonly"}]}"#,
        r#"{"planned_files":[{"path":"a","allowed_change":7}]}"#,
        r#"{"planned_files":["/etc/passwd"]}"#,
        r#"{"planned_files":["C:/Windows/x"]}"#,
        r#"{"planned_files":["../outside"]}"#,
        r#"{"planned_files":["a/../../b"]}"#,
        r#"{"planned_files":["a\\b"]}"#,
        r#"{"planned_files":["a\nb"]}"#,
        r#"{"planned_files":["a\u202eb"]}"#,
        r#"{"planned_files":[" "]}"#,
        r#"{"planned_files":[3]}"#,
        r#"{"planned_files":[{"path":"a","extra":1}]}"#,
        r#"{"planned_files":"a"}"#,
        r#"{"nope":[]}"#,
        r#"{"forbidden_changes":"x"}"#,
        r#"{"expected_tests":[1]}"#,
        r#"{"expected_tests":["go test\n"]}"#,
        r#""a string""#,
        r#"[]"#,
    ];
    for scope in cases {
        refused(&store, "BAD", scope);
    }
    assert!(
        store.task("BAD").unwrap().is_none(),
        "a refused scope still created the item"
    );
}

#[test]
fn a_nul_or_a_duplicate_key_in_a_scope_is_refused_at_the_request_boundary() {
    let store = Store::open_in_memory().unwrap();
    store.ensure_local_repository().unwrap();
    let request = item_request("NUL", "ready", r#"{"planned_files":["a\u0000b"]}"#);
    let error = store.workbench_request("items.put", &request).unwrap_err();
    assert!(error.message.contains("NUL"), "{}", error.message);
    assert!(store.task("NUL").unwrap().is_none());
    // Duplicate keys are ambiguous (which one wins differs by decoder).
    let request = item_request(
        "DUP",
        "ready",
        r#"{"planned_files":[{"path":"a","path":"b"}]}"#,
    );
    let error = store.workbench_request("items.put", &request).unwrap_err();
    assert!(error.message.contains("duplicate"), "{}", error.message);
}

#[test]
fn scope_arrays_and_entries_are_bounded() {
    let store = Store::open_in_memory().unwrap();
    let many: Vec<String> = (0..1025).map(|i| format!("\"f{i}\"")).collect();
    refused(
        &store,
        "BIG",
        &format!(r#"{{"planned_files":[{}]}}"#, many.join(",")),
    );
    let at_limit: Vec<String> = (0..1024).map(|i| format!("\"f{i}\"")).collect();
    put(
        &store,
        "OK",
        &format!(r#"{{"planned_files":[{}]}}"#, at_limit.join(",")),
    )
    .expect("1024 entries are within the bound");
    refused(
        &store,
        "LONG",
        &format!(r#"{{"expected_tests":["{}"]}}"#, "x".repeat(4097)),
    );
    refused(
        &store,
        "LONGPATH",
        &format!(r#"{{"planned_files":["{}"]}}"#, "p".repeat(1025)),
    );
    // Many long entries: each within its bound, the whole past the ceiling.
    let wide: Vec<String> = (0..80)
        .map(|_| format!("\"{}\"", "w".repeat(4000)))
        .collect();
    refused(
        &store,
        "WIDE",
        &format!(
            r#"{{"expected_tests":[{}],"allowed_commands":[{}],"forbidden_changes":[{}]}}"#,
            wide.join(","),
            wide.join(","),
            wide.join(",")
        ),
    );
}

#[test]
fn an_update_that_omits_scope_keeps_it_and_an_empty_object_clears_it() {
    let store = Store::open_in_memory().unwrap();
    put(&store, "K", r#"{"planned_files":["src/a.rs"]}"#).unwrap();
    // A board edit that knows nothing about scope.
    store
        .workbench_request(
            "items.put",
            r#"{"request_id":"edit-1","id":"K","expected_revision":1,"title":"renamed","status":"ready","repository_ids":["this-repo"],"primary_repository_id":"this-repo"}"#,
        )
        .unwrap();
    assert_eq!(
        store.task("K").unwrap().unwrap().planned_files_json,
        r#"["src/a.rs"]"#,
        "an edit that omitted scope erased it"
    );
    store
        .workbench_request(
            "items.put",
            r#"{"request_id":"edit-2","id":"K","expected_revision":2,"title":"renamed","status":"ready","repository_ids":["this-repo"],"primary_repository_id":"this-repo","scope":{}}"#,
        )
        .unwrap();
    assert_eq!(store.task("K").unwrap().unwrap().planned_files_json, "[]");
}

// --- ready / task / acquire read items ----------------------------------------

#[test]
fn ready_lists_only_live_ready_items_of_this_repository_without_a_lease() {
    let store = Store::open_in_memory().unwrap();
    assert!(
        ids(&store, ReadyMode::Enforce).is_empty(),
        "a store with no items has no ready work"
    );
    plant(&store, "READY", "ready", "{}");
    plant(&store, "BACKLOG", "backlog", "{}");
    plant(&store, "PROGRESS", "in_progress", "{}");
    plant(&store, "REVIEW", "review", "{}");
    plant(&store, "DONE", "done", "{}");
    plant(&store, "ARCHIVED", "ready", "{}");
    plant(&store, "DELETED", "ready", "{}");
    plant(&store, "LEASED", "ready", "{}");
    store
        .workbench_request(
            "repositories.put",
            r#"{"request_id":"other","id":"other-repo","expected_revision":0,"name":"o","identity_key":"git:other"}"#,
        )
        .unwrap();
    store
        .workbench_request(
            "items.put",
            r#"{"request_id":"foreign","id":"FOREIGN","expected_revision":0,"title":"x","status":"ready","repository_ids":["other-repo"],"primary_repository_id":"other-repo"}"#,
        )
        .unwrap();
    store
        .workbench_request(
            "items.put",
            r#"{"request_id":"arch","id":"ARCHIVED","expected_revision":1,"title":"x","status":"ready","archived":true,"repository_ids":["this-repo"],"primary_repository_id":"this-repo"}"#,
        )
        .unwrap();
    store
        .workbench_request(
            "items.delete",
            r#"{"request_id":"del","id":"DELETED","expected_revision":1}"#,
        )
        .unwrap();
    acquire(&store, "LEASED", "someone").unwrap();

    assert_eq!(ids(&store, ReadyMode::Enforce), vec!["READY"]);
    assert!(store.task("FOREIGN").unwrap().is_none());
    assert!(store.task("DELETED").unwrap().is_none());
    assert_eq!(store.task("REVIEW").unwrap().unwrap().status, "review");
    assert_eq!(store.task("DONE").unwrap().unwrap().status, "done");
}

#[test]
fn blocked_is_derived_from_blocking_gaps_and_ordered_by_mode() {
    let store = Store::open_in_memory().unwrap();
    plant(&store, "A-001", "ready", "{}");
    plant(&store, "A-002", "ready", "{}");
    gap(&store, "G1", "A-001", true);
    gap(&store, "G2", "A-001", true);
    gap(&store, "G3", "A-002", false); // advisory finding, not a block

    assert_eq!(ids(&store, ReadyMode::Enforce), vec!["A-002"]);
    for mode in [ReadyMode::Advisory, ReadyMode::Off] {
        let ready = store.ready_tasks(mode).unwrap();
        let order: Vec<(&str, i64, bool)> = ready
            .iter()
            .map(|t| (t.id.as_str(), t.blocking_gap_count, t.ready_to_checkout()))
            .collect();
        assert_eq!(
            order,
            vec![("A-002", 0, true), ("A-001", 2, false)],
            "{mode:?}: unblocked first, then the blocked one flagged"
        );
    }
    // An explicit read of the blocked task still works and says why.
    assert_eq!(store.task("A-001").unwrap().unwrap().blocking_gap_count, 2);
    // No status was ever written: clearing the gaps unblocks it.
    store
        .connection()
        .execute("DELETE FROM gaps WHERE task_id = 'A-001'", [])
        .unwrap();
    assert_eq!(ids(&store, ReadyMode::Enforce), vec!["A-001", "A-002"]);
}

#[test]
fn ready_order_is_priority_then_position_then_id_and_a_bad_mode_is_refused() {
    let db = seeded("tasks-order");
    for (id, priority, position) in [("C", 2, 5), ("B", 1, 9), ("A", 1, 9), ("D", 3, 0)] {
        let repo = dcstore(&db, &["local-repository"]);
        assert_eq!(repo.code, 0);
        let put = dcstore(
            &db,
            &[
                "work",
                "--method",
                "items.put",
                "--input",
                &format!(
                    r#"{{"request_id":"o-{id}","id":"{id}","expected_revision":0,"title":"t","status":"ready","priority":{priority},"position":{position},"repository_ids":["this-repo"],"primary_repository_id":"this-repo"}}"#
                ),
            ],
        );
        assert!(put.stdout.contains("\"ok\":true"), "{}", put.stdout);
    }
    let ready = dcstore(&db, &["ready", "--mode", "advisory"]);
    assert!(
        ready.stdout.contains(r#""tasks":["A","B","C","D"]"#),
        "{}",
        ready.stdout
    );
    assert!(
        ready.stdout.contains(r#""ready_to_checkout":true"#),
        "{}",
        ready.stdout
    );
    let bad = dcstore(&db, &["ready", "--mode", "permissive"]);
    assert_ne!(bad.code, 0, "a mistyped mode was honoured: {}", bad.stdout);
    // No mode is the strict one.
    assert!(
        dcstore(&db, &["ready"])
            .stdout
            .contains(r#""mode":"enforce""#)
    );
}

#[test]
fn acquire_refuses_what_is_not_live_workable_work_of_this_repository() {
    let store = Store::open_in_memory().unwrap();
    plant(&store, "GO", "ready", "{}");
    plant(&store, "STARTED", "in_progress", "{}");
    plant(&store, "REVIEW", "review", "{}");
    plant(&store, "GONE", "ready", "{}");
    plant(&store, "ARCH", "ready", "{}");

    let first = acquire(&store, "GO", "a").expect("a ready item is leasable");
    assert_eq!(first.task_id, "GO");
    acquire(&store, "STARTED", "a").expect("an item already started is leasable again");
    for (id, code) in [("REVIEW", "task_not_ready"), ("NOPE", "no_such_task")] {
        match acquire(&store, id, "a") {
            Err(StoreError::TaskUnavailable { code: got, .. }) => assert_eq!(got, code, "{id}"),
            other => panic!("{id}: {other:?}"),
        }
    }

    // Archived or deleted between `ready` and `acquire`.
    assert!(ids(&store, ReadyMode::Enforce).contains(&"GONE".to_string()));
    assert!(ids(&store, ReadyMode::Enforce).contains(&"ARCH".to_string()));
    store
        .workbench_request(
            "items.delete",
            r#"{"request_id":"d","id":"GONE","expected_revision":1}"#,
        )
        .unwrap();
    store
        .workbench_request(
            "items.put",
            r#"{"request_id":"a","id":"ARCH","expected_revision":1,"title":"x","status":"ready","archived":true,"repository_ids":["this-repo"],"primary_repository_id":"this-repo"}"#,
        )
        .unwrap();
    for (id, code) in [("GONE", "no_such_task"), ("ARCH", "task_archived")] {
        match acquire(&store, id, "b") {
            Err(StoreError::TaskUnavailable { code: got, .. }) => assert_eq!(got, code, "{id}"),
            other => panic!("{id} was leased after it was retired: {other:?}"),
        }
    }
    let leases = store.active_leases().unwrap();
    assert!(
        leases
            .iter()
            .all(|l| l.task_id != "GONE" && l.task_id != "ARCH"),
        "a lease was written for a retired item"
    );
}

#[test]
fn a_lease_that_lapses_frees_its_item_and_a_second_acquire_wins() {
    let mut store = Store::open_in_memory().unwrap();
    let now = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(1_700_000_000));
    let clock = now.clone();
    store.set_clock(move || clock.load(std::sync::atomic::Ordering::SeqCst));
    plant(&store, "T", "ready", "{}");
    let first = store
        .acquire(&AcquireRequest {
            task_id: "T".into(),
            owner: "a".into(),
            ttl_seconds: Some(60),
            ..Default::default()
        })
        .unwrap();
    now.store(1_700_000_061, std::sync::atomic::Ordering::SeqCst);
    let second = acquire(&store, "T", "b").expect("the lapsed lease did not block a new holder");
    assert_ne!(first.id, second.id);
    assert_eq!(
        store.diagnose("T", &first.token).unwrap(),
        dc_store::LeaseCode::HeldByOther,
        "the lapsed holder's token must not read as the new lease"
    );
}

#[test]
fn two_processes_racing_for_one_item_produce_one_lease() {
    let db = seeded("tasks-race");
    plant_db(&db, "RACE", "ready", "{}");
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let db = db.clone();
            std::thread::spawn(move || {
                dcstore(
                    &db,
                    &[
                        "acquire",
                        "--task",
                        "RACE",
                        "--owner",
                        &format!("p{i}"),
                        "--ttl-seconds",
                        "300",
                    ],
                )
            })
        })
        .collect();
    let replies: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let winners = replies
        .iter()
        .filter(|r| r.stdout.contains("\"ok\":true"))
        .count();
    assert_eq!(
        winners,
        1,
        "{:?}",
        replies.iter().map(|r| &r.stdout).collect::<Vec<_>>()
    );
    assert!(
        replies
            .iter()
            .filter(|r| !r.stdout.contains("\"ok\":true"))
            .all(|r| r.stdout.contains("lease_held_by_other")),
        "a loser reported something other than the conflict"
    );
}

#[test]
fn a_locked_database_is_an_error_not_an_empty_ready_list() {
    let db = seeded("tasks-locked");
    plant_db(&db, "T", "ready", "{}");
    let holder = Connection::open(&db).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let reply = dcstore(&db, &["ready"]);
    holder.execute_batch("ROLLBACK").unwrap();
    assert_ne!(reply.code, 0, "{}", reply.stdout);
    assert!(
        !reply.stdout.contains("\"tasks\":[]"),
        "a locked store answered as though it had no work: {}",
        reply.stdout
    );
}

#[test]
fn appended_scope_survives_a_board_edit_of_the_item() {
    let store = Store::open_in_memory().unwrap();
    put(&store, "W", r#"{"planned_files":["src/a.rs"]}"#).unwrap();
    let lease = acquire(&store, "W", "a").unwrap();
    store
        .set_agent_appended_planned_files("W", &lease.token, "[]", r#"["src/b.rs"]"#)
        .unwrap();
    store
        .workbench_request(
            "items.put",
            r#"{"request_id":"edit","id":"W","expected_revision":1,"title":"edited","status":"in_progress","repository_ids":["this-repo"],"primary_repository_id":"this-repo"}"#,
        )
        .unwrap();
    let task = store.task("W").unwrap().unwrap();
    assert_eq!(task.planned_files_json, r#"["src/a.rs","src/b.rs"]"#);
    assert_eq!(task.agent_appended_planned_files_json, r#"["src/b.rs"]"#);
}

#[test]
fn a_planted_scope_that_is_not_an_array_is_reported_not_spliced() {
    let store = Store::open_in_memory().unwrap();
    plant(&store, "P", "ready", "{}");
    store
        .connection()
        .execute(
            "UPDATE work_items SET body = json_set(body, '$.scope.planned_files', '[1],\"x\":[2')",
            [],
        )
        .unwrap();
    assert!(matches!(store.task("P"), Err(StoreError::BadScope { .. })));
}

// --- the legacy tasks table is archived into items ------------------------------

/// A store as schema 9 left it: the legacy table with rows in each status, a
/// live lease, a gap and a run that all name tasks by id.
fn pre_migration(name: &str) -> std::path::PathBuf {
    let path = temp_dir(name).join("state.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL").ok();
    conn.execute_batch(schema::SCHEMA).unwrap();
    conn.execute_batch(LEGACY_TASKS).unwrap();
    conn.execute_batch(
        r#"INSERT INTO tasks (id, title, description, status, priority, difficulty, planned_files_json,
                              requirement_ids_json, acceptance_criterion_ids_json, expected_tests_json,
                              allowed_commands_json, forbidden_changes_json,
                              agent_appended_planned_files_json)
           VALUES
            ('TASK-1', 'finished', 'd1', 'done', 'high', 'easy',
             '[{"path":"a.rs","reason":"why","allowed_change":"modify"}]', '["REQ-1"]', '["AC-1"]',
             '["cargo test"]', '["cargo"]', '["**/.env"]', '[]'),
            ('TASK-2', 'verified one', '', 'verified', 'low', NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]'),
            ('TASK-3', 'open', '', 'ready', 'critical', 'hard', '["src/x.rs"]', '[]', '[]', '[]', '[]', '[]',
             '[{"path":"src/y.rs","allowed_change":"create"}]'),
            ('TASK-4', 'planned', '', 'planned', NULL, NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]'),
            ('TASK-5', 'was blocked', '', 'blocked', 'medium', NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]'),
            ('TASK-6', 'mid-flight', '', 'in_progress', NULL, NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]'),
            ('TASK-7', 'odd status', '', 'someday', NULL, NULL, '[]', '[]', '[]', '[]', '[]', '[]', '[]');
           INSERT INTO task_leases (id, task_id, owner, lease_token, status, created_at, expires_at)
             VALUES ('L1', 'TASK-6', 'agent', 'tok', 'active', '2999-01-01T00:00:00+00:00', '2999-01-01T00:00:00+00:00');
           INSERT INTO gaps (id, severity, gap_type, description, recommended_fix, blocking, task_id)
             VALUES ('G1', 'high', 'unproven', 'd', 'f', 1, 'TASK-3');
           INSERT INTO evidence (type, task_id, data_json) VALUES ('test', 'TASK-1', '{}');
           INSERT INTO schema_version (id, version) VALUES ('singleton', 9);"#,
    )
    .unwrap();
    path
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
    .unwrap()
}

fn item_state(conn: &Connection, id: &str) -> (String, i64) {
    conn.query_row(
        "SELECT status, archived FROM work_items WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or_else(|e| panic!("{id} is not an item: {e}"))
}

#[test]
fn the_legacy_tasks_table_is_archived_as_items_and_dropped() {
    let path = pre_migration("tasks-migrate");
    let store = Store::open(&path).unwrap();
    let conn = store.connection();

    assert!(!table_exists(conn, "tasks"), "the legacy table survived");
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, i64::from(SCHEMA_VERSION));
    assert_eq!(SCHEMA_VERSION, 11);

    assert_eq!(item_state(conn, "TASK-1"), ("done".into(), 1));
    assert_eq!(item_state(conn, "TASK-2"), ("done".into(), 1));
    assert_eq!(item_state(conn, "TASK-3"), ("ready".into(), 0));
    assert_eq!(item_state(conn, "TASK-4"), ("ready".into(), 0));
    assert_eq!(item_state(conn, "TASK-5"), ("ready".into(), 0));
    assert_eq!(item_state(conn, "TASK-6"), ("in_progress".into(), 0));
    assert_eq!(
        item_state(conn, "TASK-7"),
        ("backlog".into(), 0),
        "a status nothing places must not become ready work"
    );

    // Scope, links and difficulty came across; the appended scope kept its place.
    let t1 = store.task("TASK-1").unwrap().unwrap();
    assert!(t1.planned_files_json.contains(r#""path":"a.rs""#));
    assert_eq!(t1.requirement_ids_json, r#"["REQ-1"]"#);
    assert_eq!(t1.acceptance_criterion_ids_json, r#"["AC-1"]"#);
    assert_eq!(t1.forbidden_changes_json, r#"["**/.env"]"#);
    assert_eq!(t1.expected_tests_json, r#"["cargo test"]"#);
    assert_eq!(t1.difficulty.as_deref(), Some("easy"));
    let t3 = store.task("TASK-3").unwrap().unwrap();
    assert_eq!(
        t3.agent_appended_planned_files_json,
        r#"[{"path":"src/y.rs","allowed_change":"create"}]"#
    );
    assert!(
        t3.planned_files_json.contains("src/x.rs") && t3.planned_files_json.contains("src/y.rs")
    );
    assert_eq!(t3.blocking_gap_count, 1);

    // The strings other tables hold still resolve.
    let lease = store
        .active_lease("TASK-6")
        .unwrap()
        .expect("the lease survived");
    assert_eq!(lease.id, "L1");
    let gaps: i64 = conn
        .query_row(
            "SELECT count(*) FROM gaps WHERE task_id = 'TASK-3'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gaps, 1);
    let evidence: i64 = conn
        .query_row(
            "SELECT count(*) FROM evidence WHERE task_id = 'TASK-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(evidence, 1);

    // What is offered: the open ones, the blocked-by-gap one last.
    assert_eq!(
        ids(&store, ReadyMode::Advisory),
        vec!["TASK-4", "TASK-5", "TASK-3"]
    );
    assert_eq!(ids(&store, ReadyMode::Enforce), vec!["TASK-4", "TASK-5"]);

    // A backup of the store as it was found was taken before the table went.
    let backup = format!("{}.pre-tasks-retire.bak", path.display());
    assert!(Path::new(&backup).exists(), "no backup at {backup}");
    let old = Connection::open(&backup).unwrap();
    assert!(table_exists(&old, "tasks"));
}

#[test]
fn migrating_twice_changes_nothing() {
    let path = pre_migration("tasks-migrate-twice");
    let snapshot = |store: &Store| -> (i64, i64, String) {
        let c = store.connection();
        (
            c.query_row("SELECT count(*) FROM work_items", [], |r| r.get(0)).unwrap(),
            c.query_row("SELECT count(*) FROM work_events", [], |r| r.get(0)).unwrap(),
            c.query_row(
                "SELECT group_concat(id || ':' || revision || ':' || body, '|') FROM (SELECT * FROM work_items ORDER BY id)",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        )
    };
    let first = snapshot(&Store::open(&path).unwrap());
    let second = snapshot(&Store::open(&path).unwrap());
    assert_eq!(first, second);
    assert_eq!(first.0, 7);
}

#[test]
fn a_half_migrated_store_is_finished_not_duplicated() {
    let path = pre_migration("tasks-migrate-half");
    drop(Store::open(&path).unwrap());
    // Rebuild the state an interrupted attempt leaves: the legacy table still
    // holding every row, stamped 9, beside only some of the items.
    let backup = format!("{}.pre-tasks-retire.bak", path.display());
    let conn = Connection::open(&path).unwrap();
    conn.execute("ATTACH DATABASE ?1 AS old", [&backup])
        .unwrap();
    conn.execute_batch(LEGACY_TASKS).unwrap();
    conn.execute_batch(
        "INSERT INTO tasks SELECT * FROM old.tasks;
         DETACH DATABASE old;
         PRAGMA foreign_keys=OFF;
         DELETE FROM work_item_repositories WHERE item_id IN ('TASK-5','TASK-6','TASK-7');
         DELETE FROM work_items WHERE id IN ('TASK-5','TASK-6','TASK-7');
         DELETE FROM work_revisions WHERE entity_id IN ('TASK-5','TASK-6','TASK-7');
         DELETE FROM work_requests WHERE id IN ('legacy-task-TASK-5','legacy-task-TASK-6','legacy-task-TASK-7');
         UPDATE schema_version SET version = 9;",
    )
    .unwrap();
    let before: i64 = conn
        .query_row("SELECT count(*) FROM work_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, 4);
    drop(conn);

    let finished = Store::open(&path).unwrap();
    assert!(!table_exists(finished.connection(), "tasks"));
    let after: i64 = finished
        .connection()
        .query_row("SELECT count(*) FROM work_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, 7, "the finish duplicated or dropped items");
    assert_eq!(item_state(finished.connection(), "TASK-6").0, "in_progress");
    let version: i64 = finished
        .connection()
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, i64::from(SCHEMA_VERSION));
}

#[test]
fn a_row_that_cannot_become_an_item_leaves_the_store_as_found() {
    let path = pre_migration("tasks-migrate-refuse");
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE tasks SET planned_files_json = '[\"../escape\"]' WHERE id = 'TASK-3'",
            [],
        )
        .unwrap();
    let err = Store::open(&path).expect_err("an unmigratable row must refuse the open");
    let message = err.to_string();
    assert!(
        message.contains("TASK-3") && message.contains(".."),
        "the refusal must name the task and the reason: {message}"
    );
    let conn = Connection::open(&path).unwrap();
    assert!(table_exists(&conn, "tasks"), "the table was dropped anyway");
    let items: i64 = conn
        .query_row("SELECT count(*) FROM work_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(items, 0, "a partial archive was committed");
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, 9,
        "the version was stamped over an unfinished migration"
    );

    // An id the workbench cannot hold is the same refusal, not a skipped row.
    let path = pre_migration("tasks-migrate-bad-id");
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE tasks SET id = 'TASK 3' WHERE id = 'TASK-3'", [])
        .unwrap();
    assert!(Store::open(&path).is_err());
}

#[test]
fn a_store_that_still_needs_migrating_is_not_read_as_empty() {
    let path = pre_migration("tasks-readonly");
    let err = Store::open_readonly(&path).expect_err("a reader cannot migrate");
    assert!(err.to_string().contains("legacy tasks table"), "{err}");
    Store::open(&path).unwrap();
    Store::open_readonly(&path).expect("a migrated store reads");
}

#[test]
fn an_older_reader_fails_closed_and_a_newer_one_migrates() {
    let path = pre_migration("tasks-versions");
    let conn = Connection::open(&path).unwrap();
    // A build that knows an older schema reads the stamp this migration
    // leaves (the current one) and refuses it, before touching a table it
    // cannot read. Both 9 (before the tasks table was retired) and 10 (before
    // reviews) are such builds.
    drop(Store::open(&path).unwrap());
    for older in [9, SCHEMA_VERSION - 1] {
        assert!(
            schema::ensure_schema_version(&conn, older)
                .unwrap_err()
                .contains(&format!(
                    "unsupported DevCouncil schema version {SCHEMA_VERSION}"
                ))
        );
    }
    // This build refuses what is newer than it, and leaves it untouched.
    conn.execute(
        "UPDATE schema_version SET version = ?1",
        [SCHEMA_VERSION + 1],
    )
    .unwrap();
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::SchemaTooNew { found, known })
            if found == SCHEMA_VERSION + 1 && known == SCHEMA_VERSION
    ));
}

#[test]
fn a_v6_shaped_legacy_table_with_missing_columns_still_migrates() {
    let path = temp_dir("tasks-v6").join("state.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (id VARCHAR NOT NULL, title VARCHAR NOT NULL, description VARCHAR NOT NULL,
            planned_files_json VARCHAR NOT NULL DEFAULT '[]', status VARCHAR NOT NULL DEFAULT 'planned',
            PRIMARY KEY (id));
         INSERT INTO tasks (id, title, description, status, planned_files_json)
            VALUES ('OLD-1', 't', '', 'ready', '[\"a.rs\"]');
         CREATE TABLE schema_version (id VARCHAR NOT NULL, version INTEGER NOT NULL, PRIMARY KEY (id));
         INSERT INTO schema_version VALUES ('singleton', 6);",
    )
    .unwrap();
    drop(conn);
    let store = Store::open(&path).unwrap();
    let task = store.task("OLD-1").unwrap().expect("a v6 task migrates");
    assert_eq!(task.planned_files_json, r#"["a.rs"]"#);
    assert_eq!(ids(&store, ReadyMode::Enforce), vec!["OLD-1"]);
}

#[test]
fn a_store_with_zero_legacy_rows_just_drops_the_empty_table() {
    let path = temp_dir("tasks-empty").join("state.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(schema::SCHEMA).unwrap();
    conn.execute_batch(LEGACY_TASKS).unwrap();
    conn.execute("UPDATE schema_version SET version = 9", [])
        .ok();
    conn.execute(
        "INSERT OR REPLACE INTO schema_version VALUES ('singleton', 9)",
        [],
    )
    .unwrap();
    drop(conn);
    let store = Store::open(&path).unwrap();
    assert!(!table_exists(store.connection(), "tasks"));
    assert_eq!(
        stamped_version(store.connection()),
        i64::from(SCHEMA_VERSION)
    );
    // Opening is a read for hosts that share the profile: an empty legacy table
    // gives it nothing to receive, so the open must not create or upgrade it.
    assert!(
        !table_exists(store.connection(), "work_items"),
        "retiring an empty legacy table created the workbench profile"
    );
    assert!(ids(&store, ReadyMode::Enforce).is_empty());
    assert!(
        !Path::new(&format!("{}.pre-tasks-retire.bak", path.display())).exists(),
        "an empty table needs no backup"
    );
}

// --- the retirement and the schema stamp commit together ----------------------

fn stamped_version(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT version FROM schema_version WHERE id = 'singleton'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn a_failed_stamp_leaves_the_legacy_table_and_its_version_as_found() {
    let path = pre_migration("tasks-stamp-fails");
    let refused = Store::open_failing_stamp_after(&path, 0)
        .expect_err("an injected stamp failure must refuse the open");
    assert!(
        matches!(
            refused,
            StoreError::Workbench {
                code: "legacy_task_unmigratable",
                ..
            }
        ),
        "{refused}"
    );
    {
        // As found: an older reader still sees version 9 with its tasks table
        // and every row, never version 9 with the table gone.
        let conn = Connection::open(&path).unwrap();
        assert!(
            table_exists(&conn, "tasks"),
            "the drop committed without the stamp"
        );
        let held: i64 = conn
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(held, 7);
        assert_eq!(stamped_version(&conn), 9);
        let items: i64 = conn
            .query_row("SELECT count(*) FROM work_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(items, 0, "items committed without the stamp");
    }
    let store = Store::open(&path).expect("the next open finishes the retirement");
    assert!(!table_exists(store.connection(), "tasks"));
    assert_eq!(
        stamped_version(store.connection()),
        i64::from(SCHEMA_VERSION)
    );
    assert_eq!(item_state(store.connection(), "TASK-1").0, "done");
}
