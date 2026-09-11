//! Stress: schema_version refuse, evidence verbs, and lease race under WAL.
//!
//! Characterises fail-closed behaviour the Go client depends on after Phase 3
//! extended the Python-owned tables into this crate.

use dc_store::{AcquireRequest, SCHEMA_VERSION, Store, schema};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn schema_newer_than_known_is_refused() {
    let store = Store::open_in_memory().expect("open");
    store
        .connection()
        .execute(
            "UPDATE schema_version SET version = ?1 WHERE id = 'singleton'",
            [(SCHEMA_VERSION as i64) + 7],
        )
        .expect("bump");
    let err = schema::ensure_schema_version(store.connection(), SCHEMA_VERSION).unwrap_err();
    assert!(
        err.contains("unsupported"),
        "expected unsupported schema refusal, got {err}"
    );
}

#[test]
fn evidence_run_and_handoff_round_trip() {
    let store = Store::open_in_memory().expect("open");
    let id = store
        .evidence_append("test", Some("T-1"), None, None, r#"{"ok":true}"#)
        .expect("evidence");
    assert!(id > 0);

    let run = dc_store::records::VerificationRun {
        id: "run-1".into(),
        task_id: "T-1".into(),
        sandbox: "local".into(),
        environment_json: "{}".into(),
        commands_json: "[]".into(),
        status: "passed".into(),
        started_at: "2026-01-01T00:00:00Z".into(),
        finished_at: Some("2026-01-01T00:00:01Z".into()),
    };
    store.run_record(&run).expect("run");
    assert_eq!(store.run_get("run-1").unwrap().unwrap().status, "passed");

    let handoff = dc_store::records::AgentHandoff {
        id: "h-1".into(),
        task_id: "T-1".into(),
        from_agent: "planner".into(),
        to_agent: "builder".into(),
        run_id: "run-1".into(),
        manifest_path: "manifest.json".into(),
        status: "complete".into(),
        created_at: "2026-01-01T00:00:02Z".into(),
    };
    store.handoff_record(&handoff).expect("handoff");
    assert_eq!(
        store.handoff_get("h-1").unwrap().unwrap().to_agent,
        "builder"
    );
}

#[test]
fn concurrent_acquire_elects_exactly_one_holder() {
    let dir = tempfile_dir();
    let db = dir.join("state.sqlite");
    {
        let store = Store::open(&db).expect("create");
        // Seed a task row so ready/task paths stay coherent under contention.
        store
            .connection()
            .execute(
                "INSERT INTO tasks (id, title, description) VALUES ('RACE', 'r', 'd')",
                [],
            )
            .expect("seed");
    }

    let barrier = Arc::new(Barrier::new(16));
    let mut handles = Vec::new();
    for i in 0..16 {
        let db = db.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let store = Store::open(&db).expect("open");
            barrier.wait();
            store.acquire(&AcquireRequest {
                task_id: "RACE".into(),
                owner: format!("owner-{i}"),
                ttl_seconds: Some(60),
                ..Default::default()
            })
        }));
    }

    let mut winners = 0;
    for h in handles {
        if h.join().unwrap().is_ok() {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one acquire must win under WAL contention"
    );
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dc-store-stress-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    dir
}
