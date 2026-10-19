//! Integrity of what the store holds and hands back.
//!
//! Every case here failed against the store as it stood before this suite: raw
//! JSON columns spliced into replies, one unreadable expiry taking the store
//! down, lazy expiry racing renewal, a pre-v7 store stamped current without its
//! columns, and non-atomic force-acquire.

use dc_store::json::{check_array, check_object};
use dc_store::{AcquireRequest, ScopeWrite, Store, schema};
use rusqlite::{Connection, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};

#[allow(dead_code)]
mod support;
use support::{dcstore, plant, seeded, temp_dir};

const INJECTION: &str = r#"[]}],"gaps":[],"x":[{"q":0"#;

fn gap(db: &std::path::Path, id: &str, evidence: Option<&str>) -> support::Reply {
    let mut args = vec![
        "gap-upsert",
        "--id",
        id,
        "--task",
        "T1",
        "--gap-type",
        "unproven",
        "--description",
        "d",
        "--blocking",
        "true",
    ];
    if let Some(e) = evidence {
        args.push("--evidence");
        args.push(e);
    }
    dcstore(db, &args)
}

// --- raw JSON splicing ------------------------------------------------------

#[test]
fn a_gap_evidence_that_closes_the_array_is_refused_on_write() {
    let db = seeded("integrity-gap-write");
    assert_eq!(gap(&db, "G1", None).code, 0);
    let refused = gap(&db, "G2", Some(INJECTION));
    assert_ne!(
        refused.code, 0,
        "an injecting evidence value was stored: {}",
        refused.stdout
    );
    let reply = dcstore(&db, &["gaps", "--task", "T1"]);
    assert!(reply.stdout.contains("\"shown\":1"), "{}", reply.stdout);
}

/// A row some other writer left behind (the file is shared) must neither hide
/// a blocking gap nor break the reply.
#[test]
fn a_planted_gap_evidence_cannot_hide_a_blocking_gap_on_read() {
    let db = seeded("integrity-gap-read");
    assert_eq!(gap(&db, "G1", None).code, 0);
    assert_eq!(gap(&db, "G2", None).code, 0);
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE gaps SET evidence_json = ?1 WHERE id = 'G2'",
            [INJECTION],
        )
        .unwrap();
    let reply = dcstore(&db, &["gaps", "--task", "T1"]);
    assert_eq!(reply.code, 0, "{}", reply.stdout);
    check_object(&reply.stdout)
        .unwrap_or_else(|why| panic!("reply is not one object: {why}: {}", reply.stdout));
    assert!(reply.stdout.contains("\"shown\":2"), "{}", reply.stdout);
    assert!(reply.stdout.contains("\"id\":\"G1\""), "{}", reply.stdout);
    assert!(
        reply.stdout.contains("\"evidence_error\""),
        "{}",
        reply.stdout
    );
    assert!(
        !reply.stdout.contains("\"x\":["),
        "injected keys reached the reply: {}",
        reply.stdout
    );
}

#[test]
fn evidence_data_is_fully_scanned_on_write_and_read() {
    let db = seeded("integrity-evidence");
    for bad in ["{", "[", r#"{"a":1}}"#, r#"[1],"x":[2"#, "{} {}"] {
        let r = dcstore(&db, &["evidence-append", "--type", "t", "--data", bad]);
        assert_ne!(r.code, 0, "{bad:?} was stored");
    }
    assert_eq!(
        dcstore(
            &db,
            &["evidence-append", "--type", "t", "--data", r#"{"ok":[1]}"#]
        )
        .code,
        0
    );
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO evidence (type, data_json) VALUES ('t', '{')",
            [],
        )
        .unwrap();
    let reply = dcstore(&db, &["evidence-list"]);
    check_object(&reply.stdout)
        .unwrap_or_else(|why| panic!("evidence-list is not one object: {why}: {}", reply.stdout));
    assert!(reply.stdout.contains("\"data_error\""), "{}", reply.stdout);
}

#[test]
fn run_record_environment_and_commands_are_validated() {
    let db = seeded("integrity-run");
    let base = |env: &'static str, cmds: &'static str| {
        dcstore(
            &db,
            &[
                "run-record",
                "--id",
                "R1",
                "--task",
                "T1",
                "--sandbox",
                "s",
                "--status",
                "ok",
                "--started-at",
                "2026-01-01T00:00:00+00:00",
                "--environment",
                env,
                "--commands",
                cmds,
            ],
        )
    };
    assert_ne!(base("{", "[]").code, 0);
    assert_ne!(base("{}", r#"[]}],"a":[{"b":0"#).code, 0);
    assert_ne!(base("[]", "[]").code, 0, "an array is not an environment");
    assert_eq!(base(r#"{"A":"1"}"#, r#"["go test"]"#).code, 0);
}

fn store_with_task(name: &str) -> (Store, std::path::PathBuf) {
    let path = temp_dir(name).join("state.sqlite");
    let store = Store::open(&path).unwrap();
    plant(&store, "T1", "ready", "{}");
    (store, path)
}

fn ready_ids(store: &Store) -> Vec<String> {
    store
        .ready_tasks(dc_store::ReadyMode::Enforce)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}

fn lease_for(store: &Store, task: &str, ttl: Option<i64>) -> dc_store::Lease {
    store
        .acquire(&AcquireRequest {
            task_id: task.into(),
            owner: "o".into(),
            ttl_seconds: ttl,
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn appended_scope_padded_with_non_json_whitespace_is_refused_not_stored() {
    let (store, _) = store_with_task("integrity-ws-refuse");
    let lease = lease_for(&store, "T1", Some(900));
    for bad in ["\x0c[]", "[]\x0c", "\u{a0}[]", "[]\u{a0}", "\u{2003}[]"] {
        assert!(
            store
                .set_agent_appended_planned_files("T1", &lease.token, "[]", bad)
                .is_err(),
            "{bad:?} was stored"
        );
    }
    let task = store
        .task("T1")
        .unwrap()
        .expect("a refused write must leave the task readable");
    assert_eq!(task.agent_appended_planned_files_json, "[]");
}

#[test]
fn newline_padded_scope_is_normalised_and_swappable() {
    let (store, _) = store_with_task("integrity-ws-cas");
    let lease = lease_for(&store, "T1", Some(900));
    let first = "\n[{\"path\":\"a.go\",\"allowed_change\":\"modify\"}]\r\n";
    assert_eq!(
        store
            .set_agent_appended_planned_files("T1", &lease.token, "[]", first)
            .unwrap(),
        ScopeWrite::Written
    );
    // A store left by another writer with padding the old SQL trim() ignored.
    store
        .connection()
        .execute(
            "UPDATE task_scope_appends SET planned_files_json = ?1 WHERE task_id = 'T1'",
            ["\n[{\"path\":\"a.go\",\"allowed_change\":\"modify\"}]\n"],
        )
        .unwrap();
    let next =
        r#"[{"path":"a.go","allowed_change":"modify"},{"path":"b.go","allowed_change":"modify"}]"#;
    let current = r#"[{"path":"a.go","allowed_change":"modify"}]"#;
    assert_eq!(
        store
            .set_agent_appended_planned_files("T1", &lease.token, current, next)
            .unwrap(),
        ScopeWrite::Written,
        "the swap refused forever on a newline-padded column"
    );
    check_array(
        &store
            .task("T1")
            .unwrap()
            .unwrap()
            .agent_appended_planned_files_json,
    )
    .unwrap();
}

#[test]
fn a_planted_form_feed_column_is_reported_not_served() {
    let (store, _) = store_with_task("integrity-ws-read");
    store
        .connection()
        .execute(
            "INSERT OR REPLACE INTO task_scope_appends (task_id, planned_files_json) VALUES ('T1', ?1)",
            ["\x0c[]"],
        )
        .unwrap();
    assert!(
        store.task("T1").is_err(),
        "a column only a wide trim() accepts was served raw"
    );
}

// --- unreadable expiry ------------------------------------------------------

fn plant_bad_expiry(store: &Store, task: &str) {
    store
        .connection()
        .execute(
            "UPDATE task_leases SET expires_at = 'not a time' WHERE task_id = ?1",
            [task],
        )
        .unwrap();
}

#[test]
fn one_unreadable_expiry_fails_closed_for_its_own_task_only() {
    let (store, path) = store_with_task("integrity-bad-expiry");
    plant(&store, "T2", "ready", "{}");
    lease_for(&store, "T1", Some(900));
    plant_bad_expiry(&store, "T1");

    // Other tasks are served; the planted one is excluded and reported.
    assert_eq!(ready_ids(&store), vec!["T2".to_string()]);
    let bad = store.unreadable_leases().unwrap();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].task_id, "T1");
    assert!(store.active_leases().unwrap().is_empty());
    let unrelated = store.acquire(&AcquireRequest {
        task_id: "T2".into(),
        owner: "o".into(),
        ttl_seconds: Some(60),
        ..Default::default()
    });
    assert!(
        unrelated.is_ok(),
        "an unrelated task could not be acquired: {unrelated:?}"
    );

    let health = dcstore(&path, &["health"]);
    assert_eq!(health.code, 0, "{}", health.stdout);
    assert!(
        health.stdout.contains("\"degraded\":true"),
        "{}",
        health.stdout
    );
    assert!(
        health.stdout.contains("\"task_id\":\"T1\""),
        "{}",
        health.stdout
    );
    let ready = dcstore(&path, &["ready"]);
    assert_eq!(ready.code, 0, "{}", ready.stdout);
    assert!(
        ready.stdout.contains("\"unreadable_leases\":[{"),
        "{}",
        ready.stdout
    );

    // Without force the task stays locked, and the error says how to clear it.
    let plain = store.acquire(&AcquireRequest {
        task_id: "T1".into(),
        owner: "x".into(),
        ..Default::default()
    });
    assert!(plain.is_err());

    // Force retires it and records that, keeping the bad value as evidence.
    let (lease, retired) = store
        .acquire_outcome(&AcquireRequest {
            task_id: "T1".into(),
            owner: "x".into(),
            force: true,
            ..Default::default()
        })
        .unwrap();
    let retired =
        retired.expect("a forced acquire over an unreadable expiry must say what it retired");
    assert_eq!(retired.expires_at, "not a time");
    assert_eq!(lease.task_id, "T1");
    let (status, released): (String, Option<String>) = store
        .connection()
        .query_row(
            "SELECT status, released_at FROM task_leases WHERE id = ?1",
            [&retired.lease_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "stale");
    assert!(released.is_some());
    assert!(store.unreadable_leases().unwrap().is_empty());
}

// --- lazy expiry versus renewal --------------------------------------------

/// The interleaving: B reads the lease as lapsed; before B writes, A renews it.
/// B's write must lose, and A's lease must still be valid.
#[test]
fn a_stale_mark_never_overwrites_a_renewal_that_landed_first() {
    let dir = temp_dir("integrity-expiry-race");
    let path = dir.join("state.sqlite");
    let mut a = Store::open(&path).unwrap();
    a.set_clock(|| 1_700_000_000);
    plant(&a, "T1", "ready", "{}");
    let lease = lease_for(&a, "T1", Some(3600));

    let mut b = Store::open(&path).unwrap();
    let renewed = AtomicBool::new(false);
    let (rival_path, token) = (path.clone(), lease.token.clone());
    // B's clock is past the original expiry. The first time it is consulted
    // (after B has read the row) A renews, at B's own "now".
    b.set_clock(move || {
        if !renewed.swap(true, Ordering::SeqCst) {
            let mut rival = Store::open(&rival_path).unwrap();
            rival.set_clock(|| 1_700_003_000);
            assert!(rival.renew("T1", &token, 3600).unwrap().is_some());
        }
        1_700_004_000
    });
    let seen = b.active_lease("T1").unwrap();
    assert!(seen.is_some(), "B marked a freshly renewed lease stale");
    assert_eq!(
        a.diagnose("T1", &lease.token).unwrap(),
        dc_store::LeaseCode::Valid
    );
}

#[test]
fn a_released_lease_is_never_rewritten_to_stale() {
    let dir = temp_dir("integrity-released-stays");
    let path = dir.join("state.sqlite");
    let mut a = Store::open(&path).unwrap();
    a.set_clock(|| 1_700_000_000);
    plant(&a, "T1", "ready", "{}");
    let lease = lease_for(&a, "T1", Some(900));
    let mut b = Store::open(&path).unwrap();
    let released = AtomicBool::new(false);
    let (rival_path, token) = (path.clone(), lease.token.clone());
    b.set_clock(move || {
        if !released.swap(true, Ordering::SeqCst) {
            let mut rival = Store::open(&rival_path).unwrap();
            rival.set_clock(|| 1_700_000_000);
            assert!(rival.release("T1", &token).unwrap());
        }
        1_700_001_000
    });
    assert!(b.active_lease("T1").unwrap().is_none());
    let status: String = a
        .connection()
        .query_row(
            "SELECT status FROM task_leases WHERE id = ?1",
            [&lease.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "released");
}

/// Concurrent processes hammering renew, expiry reads and release: whatever the
/// interleaving, a row that was released stays released and at most one is active.
#[test]
fn concurrent_renew_expire_and_release_keep_one_consistent_row() {
    let dir = temp_dir("integrity-hammer");
    let path = dir.join("state.sqlite");
    {
        let s = Store::open(&path).unwrap();
        plant(&s, "T1", "ready", "{}");
    }
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut s = Store::open(&path).unwrap();
                let tick = std::sync::atomic::AtomicI64::new(1_700_000_000);
                s.set_clock(move || tick.fetch_add(1, Ordering::SeqCst));
                for _ in 0..40 {
                    if let Ok(l) = s.acquire(&AcquireRequest {
                        task_id: "T1".into(),
                        owner: format!("o{i}"),
                        ttl_seconds: Some(5),
                        ..Default::default()
                    }) {
                        let _ = s.renew("T1", &l.token, 5);
                        let _ = s.active_lease("T1");
                        let _ = s.release("T1", &l.token);
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    let active: i64 = conn
        .query_row(
            "SELECT count(*) FROM task_leases WHERE status='active'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(active <= 1);
    let released_without_stamp: i64 = conn
        .query_row("SELECT count(*) FROM task_leases WHERE status IN ('released','stale') AND released_at IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(released_without_stamp, 0);
}

// --- force acquire is atomic ------------------------------------------------

/// A failure at the insert must not have retired the incumbent.
#[test]
fn a_failed_forced_acquire_leaves_the_incumbent_holding() {
    let (store, _) = store_with_task("integrity-force-atomic");
    let held = lease_for(&store, "T1", Some(900));
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON task_leases WHEN NEW.owner = 'boom'
             BEGIN SELECT RAISE(ABORT, 'injected failure at the insert'); END;",
        )
        .unwrap();
    let outcome = store.acquire(&AcquireRequest {
        task_id: "T1".into(),
        owner: "boom".into(),
        force: true,
        ..Default::default()
    });
    assert!(outcome.is_err());
    assert_eq!(
        store.diagnose("T1", &held.token).unwrap(),
        dc_store::LeaseCode::Valid,
        "the incumbent was retired by a failed steal"
    );
}

// --- pre-v7 migration -------------------------------------------------------

fn v6_store(name: &str) -> std::path::PathBuf {
    let path = temp_dir(name).join("state.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
            id VARCHAR NOT NULL, title VARCHAR NOT NULL, description VARCHAR NOT NULL,
            requirement_ids_json VARCHAR NOT NULL DEFAULT '[]',
            acceptance_criterion_ids_json VARCHAR NOT NULL DEFAULT '[]',
            planned_files_json VARCHAR NOT NULL DEFAULT '[]',
            expected_tests_json VARCHAR NOT NULL DEFAULT '[]',
            allowed_commands_json VARCHAR NOT NULL DEFAULT '[]',
            forbidden_changes_json VARCHAR NOT NULL DEFAULT '[]',
            status VARCHAR NOT NULL DEFAULT 'planned', difficulty VARCHAR,
            PRIMARY KEY (id));
         INSERT INTO tasks (id, title, description, status) VALUES ('T1','t','','ready');
         -- gaps as they were before the output-path columns existed.
         CREATE TABLE gaps (
            id VARCHAR NOT NULL, severity VARCHAR NOT NULL, gap_type VARCHAR NOT NULL,
            requirement_id VARCHAR, task_id VARCHAR, description VARCHAR NOT NULL,
            evidence_json VARCHAR NOT NULL DEFAULT '[]', recommended_fix VARCHAR NOT NULL,
            blocking BOOLEAN NOT NULL, file VARCHAR, line INTEGER, suggested_command VARCHAR,
            acceptance_criterion_id VARCHAR, expected_verification_method VARCHAR,
            PRIMARY KEY (id));
         CREATE TABLE schema_version (id VARCHAR NOT NULL, version INTEGER NOT NULL, PRIMARY KEY (id));
         INSERT INTO schema_version VALUES ('singleton', 6);",
    )
    .unwrap();
    path
}

fn columns(conn: &Connection) -> Vec<String> {
    let mut s = conn.prepare("PRAGMA table_info(gaps)").unwrap();
    s.query_map([], |r| r.get::<_, String>("name"))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_v6_store_opens_with_every_column_and_reads_a_task() {
    // Its legacy `tasks` row is archived as an item on the way in, so the
    // task still reads; the columns this checks are the ones a v6 table lacks.
    let path = v6_store("integrity-v6");
    let store = Store::open(&path).unwrap();
    let task = store.task("T1").unwrap().expect("a migrated v6 task reads");
    assert_eq!(task.agent_appended_planned_files_json, "[]");
    let version: i64 = store
        .connection()
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, dc_store::SCHEMA_VERSION as i64);
    assert!(columns(store.connection()).contains(&"stdout_path".to_string()));
    assert_eq!(ready_ids(&store), vec!["T1".to_string()]);
}

/// Crash at each write step: nothing is added and nothing is stamped; a clean
/// retry then succeeds.
#[test]
fn a_migration_crashed_at_any_step_leaves_the_store_as_found() {
    // Count steps by failing at increasing budgets until one succeeds.
    let mut crashed = 0;
    for steps in 0..64 {
        let path = v6_store(&format!("integrity-v6-crash-{steps}"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(schema::SCHEMA).unwrap();
        let before = columns(&conn);
        match schema::migrate_failing_after(&conn, dc_store::SCHEMA_VERSION, steps) {
            Ok(()) => break,
            Err(_) => {
                crashed += 1;
                assert_eq!(
                    columns(&conn),
                    before,
                    "step {steps}: a crashed migration left added columns"
                );
                let version: Option<i64> = conn
                    .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
                    .optional()
                    .unwrap();
                assert_eq!(
                    version,
                    Some(6),
                    "step {steps}: a crashed migration stamped the version"
                );
                schema::migrate(&conn, dc_store::SCHEMA_VERSION)
                    .expect("a retry after a crash succeeds");
            }
        }
    }
    assert!(
        crashed >= 3,
        "the injection never reached the column steps ({crashed})"
    );
}

#[test]
fn a_store_newer_than_this_build_is_refused_before_anything_changes() {
    let path = v6_store("integrity-too-new");
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE schema_version SET version = 999", [])
        .unwrap();
    let before = columns(&conn);
    assert!(Store::open(&path).is_err());
    assert_eq!(columns(&conn), before);
}

#[test]
fn appended_scope_with_an_unknown_allowed_change_is_refused_like_the_go_side() {
    let (store, _) = store_with_task("integrity-allowed-change");
    let lease = lease_for(&store, "T1", Some(900));
    for bad in [
        r#"[{"path":"a.go","allowed_change":"destroy"}]"#,
        r#"[{"path":"a.go","allowed_change":"MODIFY"}]"#,
        r#"[{"path":"a.go","allowed_change":7}]"#,
        // Go matches keys case-insensitively and decodes escapes in them, and
        // the last duplicate wins, so each of these reaches the same field.
        r#"[{"path":"a.go","Allowed_Change":"destroy"}]"#,
        r#"[{"path":"a.go","allowed_change":"destroy"}]"#,
        r#"[{"path":"a.go","allowed_change":"modify","allowed_change":"destroy"}]"#,
        r#"[{"path":"ok.go"},{"path":"a.go","allowed_change":"delete "}]"#,
        r#"[3]"#,
    ] {
        assert!(
            store
                .set_agent_appended_planned_files("T1", &lease.token, "[]", bad)
                .is_err(),
            "{bad} was stored"
        );
    }
    for good in [
        "[]",
        r#"["src/a.go"]"#,
        r#"[{"path":"a.go"}]"#,
        r#"[{"path":"a.go","allowed_change":null}]"#,
        r#"[{"path":"a.go","allowed_change":""}]"#,
        r#"[{"path":"a.go","allowed_change":"create"},{"path":"b.go","allowed_change":"delete"},{"path":"c.go","allowed_change":"read_only"},{"path":"d.go","allowed_change":"modify"}]"#,
    ] {
        let current = store
            .task("T1")
            .unwrap()
            .unwrap()
            .agent_appended_planned_files_json;
        store
            .set_agent_appended_planned_files("T1", &lease.token, &current, good)
            .unwrap_or_else(|e| panic!("{good} refused: {e:?}"));
    }
}
