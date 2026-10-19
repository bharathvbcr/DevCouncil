//! `Store::open_readonly`: a view of the store that cannot change it.

use dc_store::{AcquireRequest, LeaseState, Store};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[allow(dead_code)]
mod support;

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dc-store-readonly-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn data_version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap()
}

fn lease(store: &Store, task: &str, ttl: i64) {
    store
        .acquire(&AcquireRequest {
            task_id: task.into(),
            owner: "o".into(),
            ttl_seconds: Some(ttl),
            ..Default::default()
        })
        .unwrap();
}

#[test]
fn a_missing_store_is_reported_not_created() {
    let dir = Scratch::new();
    let path = dir.path().join("state.sqlite");
    assert!(Store::open_readonly(&path).is_err());
    assert!(!path.exists());
}

#[test]
fn observing_leases_writes_nothing_and_reports_each_row() {
    let dir = Scratch::new();
    let path = dir.path().join("state.sqlite");
    let store = Store::open(&path).unwrap();
    for id in ["T-live", "T-lapsed", "T-bad", "T-new"] {
        support::plant(&store, id, "ready", "{}");
    }
    lease(&store, "T-live", 3600);
    lease(&store, "T-lapsed", 1);
    lease(&store, "T-bad", 3600);
    store
        .connection()
        .execute(
            "UPDATE task_leases SET expires_at='not a time' WHERE task_id='T-bad'",
            [],
        )
        .unwrap();
    store
        .connection()
        .execute(
            "UPDATE task_leases SET expires_at='2001-01-01T00:00:00Z' WHERE task_id='T-lapsed'",
            [],
        )
        .unwrap();
    drop(store);

    let reader = Store::open_readonly(&path).unwrap();
    let before = data_version(&path);
    let seen = reader.observe_leases().unwrap();
    let state = |task: &str| {
        seen.iter()
            .find(|(lease, _)| lease.task_id == task)
            .map(|(_, state)| state.clone())
            .unwrap()
    };
    assert_eq!(state("T-live"), LeaseState::Live);
    assert_eq!(state("T-lapsed"), LeaseState::Expired);
    assert_eq!(state("T-bad"), LeaseState::BadExpiry("not a time".into()));
    assert_eq!(data_version(&path), before, "observing changed the store");

    // The lapsed row is still `active`: nothing swept it.
    let status: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT status FROM task_leases WHERE task_id='T-lapsed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "active");

    // A handle opened for reading cannot write.
    assert!(
        reader
            .acquire(&AcquireRequest {
                task_id: "T-new".into(),
                owner: "o".into(),
                ttl_seconds: Some(60),
                ..Default::default()
            })
            .is_err()
    );
}

#[test]
fn a_store_from_before_the_gap_output_columns_reads_without_being_migrated() {
    let dir = Scratch::new();
    let path = dir.path().join("state.sqlite");
    let store = Store::open(&path).unwrap();
    store
        .connection()
        .execute_batch(
            "ALTER TABLE gaps DROP COLUMN stdout_path;
             ALTER TABLE gaps DROP COLUMN stderr_path;
             INSERT INTO gaps (id, severity, gap_type, description, evidence_json,
                 recommended_fix, blocking, task_id)
             VALUES ('g-old', 'high', 'coverage', 'missing test', '{}', 'add test', 1, 'T-1');",
        )
        .unwrap();
    drop(store);

    let reader = Store::open_readonly(&path).unwrap();
    let (gaps, truncated) = reader.gaps_list(Some("T-1")).unwrap();
    assert!(!truncated);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].stdout_path, None);
    let columns: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM pragma_table_info('gaps') WHERE name='stdout_path'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(columns, 0, "a reader added the column");
}
