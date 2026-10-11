//! Creating a workbench profile is atomic: a handle that opens a fresh path
//! while another is still creating it waits and then sees the finished
//! schema, never an intermediate version it would refuse as a stale profile.

use dc_store::Store;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dc-store-create-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn profile(&self) -> PathBuf {
        self.0.join("profile.sqlite")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
        .unwrap()
}

fn put_workspace(store: &Store, id: &str) -> Result<String, dc_store::workbench::Error> {
    store.workbench_request(
        "workspaces.put",
        &format!(
            r#"{{"request_id":"ws-{id}","id":"{id}","expected_revision":0,"name":"{id}","repository_ids":[]}}"#
        ),
    )
}

const THREADS: usize = 8;
const ROUNDS: usize = 40;

/// `first` runs on every thread once all of them hold an open handle on a path
/// that does not exist yet, so they all reach schema creation together.
fn race(first: impl Fn(&Store, usize) -> Result<String, dc_store::workbench::Error> + Sync) {
    for round in 0..ROUNDS {
        let dir = Scratch::new();
        let path = dir.profile();
        let barrier = Barrier::new(THREADS);
        let results: Vec<_> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..THREADS)
                .map(|n| {
                    let (path, barrier, first) = (&path, &barrier, &first);
                    scope.spawn(move || {
                        let store = Store::open(path).unwrap();
                        barrier.wait();
                        first(&store, n)
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        for (n, result) in results.iter().enumerate() {
            if let Err(error) = result {
                panic!("round {round} thread {n}: {} {error}", error.code);
            }
        }
        assert_eq!(version(&path), dc_store::workbench::WORKBENCH_SCHEMA);
    }
}

#[test]
fn racing_readers_of_a_fresh_profile_all_see_the_finished_schema() {
    race(|store, _| store.workbench_request("workspaces.list", "{}"));
}

#[test]
fn racing_writers_of_a_fresh_profile_each_commit_once() {
    race(|store, n| put_workspace(store, &format!("w{n}")));
}

#[test]
fn racing_first_registrations_of_one_id_converge_on_one_record() {
    let dir = Scratch::new();
    let path = dir.profile();
    let barrier = Arc::new(Barrier::new(THREADS));
    let workers: Vec<_> = (0..THREADS)
        .map(|_| {
            let (path, barrier) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let store = Store::open(&path).unwrap();
                barrier.wait();
                put_workspace(&store, "same")
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    for result in &results {
        if let Err(error) = result {
            assert_ne!(error.code, "schema_upgrade_required", "{error}");
        }
    }
    assert!(results.iter().any(Result::is_ok));
    let rows: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT count(*) FROM work_workspaces", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn an_empty_or_zero_byte_file_is_a_fresh_profile() {
    for (label, create) in [("zero-byte", true), ("empty sqlite", false)] {
        let dir = Scratch::new();
        let path = dir.profile();
        if create {
            std::fs::File::create(&path).unwrap();
        } else {
            drop(Connection::open(&path).unwrap());
        }
        let store = Store::open(&path).unwrap();
        store
            .workbench_request("workspaces.list", "{}")
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(
            version(&path),
            dc_store::workbench::WORKBENCH_SCHEMA,
            "{label}"
        );
    }
}

#[test]
fn an_existing_store_at_version_one_with_data_is_never_treated_as_fresh() {
    let dir = Scratch::new();
    let path = dir.profile();
    let store = Store::open(&path).unwrap();
    put_workspace(&store, "keep").unwrap();
    drop(store);
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE work_meta SET version=1 WHERE id=1", [])
        .unwrap();
    let store = Store::open(&path).unwrap();
    let error = store
        .workbench_request("workspaces.list", "{}")
        .unwrap_err();
    assert_eq!(error.code, "schema_upgrade_required", "{error}");
    assert_eq!(
        version(&path),
        1,
        "a read upgraded or recreated the profile"
    );
    let rows: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT count(*) FROM work_workspaces", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "data was lost");
}
