//! A checkout directory has one identity however it is spelled. `runs.prepare`
//! compared `git_dir` as a raw string, so a trailing slash or a `..` component
//! naming the same directory was a second checkout and missed `checkout_busy`.
//! A path that does not resolve to a directory is invalid input, never a free
//! slot.

use dc_store::Store;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

type WbResult = Result<String, dc_store::workbench::Error>;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dc-gitdir-identity-{}-{}-{label}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn quote(value: &str) -> String {
    format!("{:?}", value)
}

fn spelled(path: &Path) -> String {
    path.to_str().expect("temp paths are unicode").to_owned()
}

fn canonical(path: &Path) -> String {
    spelled(&std::fs::canonicalize(path).unwrap())
}

fn open() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let clock = Arc::new(AtomicI64::new(1000));
    store.set_clock(move || clock.load(Ordering::SeqCst));
    store
}

fn call(store: &Store, method: &str, input: &str) -> String {
    store
        .workbench_request(method, input)
        .unwrap_or_else(|error| panic!("{method}: {error}"))
}

fn text(store: &Store, raw: &str, path: &str) -> String {
    store
        .connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |row| row.get(0))
        .unwrap()
}

/// Registers repository `r` under `identity` and a task `t` linked to it.
fn register(store: &Store, identity: &str) {
    call(
        store,
        "repositories.put",
        &format!(
            r#"{{"id":"r","request_id":"repo","expected_revision":0,"name":"Repo","identity_key":{}}}"#,
            quote(identity)
        ),
    );
    call(
        store,
        "items.put",
        r#"{"id":"t","request_id":"task","expected_revision":0,"title":"Keep E42","description":"Preserve exact evidence","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
}

fn prepare(id: &str, cwd: &str, git_dir: &str, common: &str) -> String {
    format!(
        r#"{{"id":"{id}","request_id":"prepare-{id}","expected_revision":0,"task_id":"t","source_revision":1,"repository_id":"r","repository_revision":1,"provider":"codex","permission_mode":"ask","cwd":{},"git_dir":{},"git_common_dir":{},"head_oid":null}}"#,
        quote(cwd),
        quote(git_dir),
        quote(common),
    )
}

fn refused(result: WbResult) -> dc_store::workbench::Error {
    result.expect_err("the preparation must be refused")
}

#[test]
fn a_trailing_slash_on_git_dir_is_the_same_checkout_as_the_active_run() {
    let tmp = TempDir::new("slash");
    let git = tmp.dir(".git");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let plain = canonical(&git);
    call(
        &store,
        "runs.prepare",
        &prepare("first", &spelled(&tmp.0), &plain, &plain),
    );
    let slashed = format!("{plain}/");
    let error = refused(store.workbench_request(
        "runs.prepare",
        &prepare("second", &spelled(&tmp.0), &slashed, &plain),
    ));
    assert_eq!(error.code, "checkout_busy", "{error}");
}

#[test]
fn a_dotdot_component_on_git_dir_is_the_same_checkout_as_the_active_run() {
    let tmp = TempDir::new("dotdot");
    let git = tmp.dir(".git");
    tmp.dir("x");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let plain = canonical(&git);
    call(
        &store,
        "runs.prepare",
        &prepare("first", &spelled(&tmp.0), &plain, &plain),
    );
    let dotted = format!("{}/x/../.git", spelled(&tmp.0));
    let error = refused(store.workbench_request(
        "runs.prepare",
        &prepare("second", &spelled(&tmp.0), &dotted, &plain),
    ));
    assert_eq!(error.code, "checkout_busy", "{error}");
}

#[test]
fn the_stored_git_dir_is_the_resolved_spelling() {
    let tmp = TempDir::new("stored");
    let git = tmp.dir(".git");
    tmp.dir("x");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let dotted = format!("{}/x/../.git/", spelled(&tmp.0));
    let run = call(
        &store,
        "runs.prepare",
        &prepare("first", &spelled(&tmp.0), &dotted, &dotted),
    );
    assert_eq!(text(&store, &run, "$.item.git_dir"), canonical(&git));
    assert_eq!(text(&store, &run, "$.item.git_common_dir"), canonical(&git));
}

#[test]
fn a_registered_identity_in_another_spelling_still_matches() {
    let tmp = TempDir::new("identity");
    let git = tmp.dir(".git");
    tmp.dir("x");
    let store = open();
    register(&store, &format!("local:{}/x/../.git/", spelled(&tmp.0)));
    let plain = canonical(&git);
    call(
        &store,
        "runs.prepare",
        &prepare("first", &spelled(&tmp.0), &plain, &plain),
    );
}

#[test]
fn a_run_stored_before_resolution_still_blocks_a_new_run_on_the_same_directory() {
    let tmp = TempDir::new("legacy");
    let git = tmp.dir(".git");
    tmp.dir("x");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let plain = canonical(&git);
    call(
        &store,
        "runs.prepare",
        &prepare("old", &spelled(&tmp.0), &plain, &plain),
    );
    // Rows written before this change hold whatever spelling the host sent.
    let legacy = format!("{}/x/../.git/", spelled(&tmp.0));
    let changed = store
        .connection()
        .execute(
            "UPDATE work_runs SET body=json_set(body,'$.git_dir',?1) WHERE id='old'",
            [&legacy],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let error = refused(store.workbench_request(
        "runs.prepare",
        &prepare("new", &spelled(&tmp.0), &plain, &plain),
    ));
    assert_eq!(error.code, "checkout_busy", "{error}");
}

#[test]
fn a_git_dir_that_does_not_exist_is_invalid_input() {
    let tmp = TempDir::new("missing");
    let git = tmp.dir(".git");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let missing = format!("{}/absent", spelled(&tmp.0));
    let error = refused(store.workbench_request(
        "runs.prepare",
        &prepare("first", &spelled(&tmp.0), &missing, &canonical(&git)),
    ));
    assert_eq!(error.code, "invalid_input", "{error}");
}

#[test]
fn a_git_dir_that_is_a_file_is_invalid_input() {
    let tmp = TempDir::new("file");
    let git = tmp.dir(".git");
    let file = tmp.0.join("gitfile");
    std::fs::write(&file, "gitdir: elsewhere").unwrap();
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let error = refused(store.workbench_request(
        "runs.prepare",
        &prepare("first", &spelled(&tmp.0), &spelled(&file), &canonical(&git)),
    ));
    assert_eq!(error.code, "invalid_input", "{error}");
}

#[test]
fn an_active_run_whose_stored_git_dir_is_gone_is_invalid_input_not_a_free_slot() {
    let tmp = TempDir::new("gone");
    let git = tmp.dir(".git");
    let other = tmp.dir("other");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let plain = canonical(&git);
    call(
        &store,
        "runs.prepare",
        &prepare("old", &spelled(&tmp.0), &spelled(&other), &plain),
    );
    std::fs::remove_dir(&other).unwrap();
    let error = refused(store.workbench_request(
        "runs.prepare",
        &prepare("new", &spelled(&tmp.0), &plain, &plain),
    ));
    assert_eq!(error.code, "invalid_input", "{error}");
}

#[test]
fn skip_checkout_occupancy_still_lets_a_remote_run_share_a_respelled_checkout() {
    let tmp = TempDir::new("skip");
    let git = tmp.dir(".git");
    let store = open();
    register(&store, &format!("local:{}", canonical(&git)));
    let plain = canonical(&git);
    call(
        &store,
        "runs.prepare",
        &prepare("local", &spelled(&tmp.0), &plain, &plain),
    );
    let remote = prepare("remote", &spelled(&tmp.0), &format!("{plain}/"), &plain).replace(
        "\"head_oid\"",
        "\"skip_checkout_occupancy\":true,\"head_oid\"",
    );
    call(&store, "runs.prepare", &remote);
}
