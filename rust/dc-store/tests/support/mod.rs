//! Helpers shared by the integration tests that drive the real `dcstore`
//! binary.
//!
//! These lived as private functions in `boundary.rs` until a second suite
//! needed them. Copying them would have made the process boundary's test
//! harness something each suite maintains its own version of, and the first
//! divergence — a different temp-directory policy, a different definition of
//! "seeded" — would be invisible until two suites disagreed about a failure.

use std::path::{Path, PathBuf};
use std::process::Command;

const DCSTORE: &str = env!("CARGO_BIN_EXE_dcstore");

/// A directory of this test's own, named for the process and the case, so
/// concurrent tests never share a database.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-store-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

pub struct Reply {
    pub stdout: String,
    pub code: i32,
}

pub fn dcstore(db: &Path, args: &[&str]) -> Reply {
    let out = Command::new(DCSTORE)
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("run dcstore");
    Reply {
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        code: out.status.code().unwrap_or(-1),
    }
}

/// A store the other cases can talk to, created the way any writing command
/// creates one.
pub fn seeded(name: &str) -> PathBuf {
    let db = temp_dir(name).join("state.sqlite");
    let reply = dcstore(&db, &["ready"]);
    assert_eq!(reply.code, 0, "seeding the store: {}", reply.stdout);
    db
}

/// The `items.put` request that makes one task: a workbench item of this
/// repository, with the status and scope object a test names. Items are the
/// only way a task comes to exist now, so every suite plants through here.
pub fn item_request(id: &str, status: &str, scope: &str) -> String {
    format!(
        r#"{{"request_id":"plant-{id}","id":"{id}","expected_revision":0,"title":"planted {id}","status":"{status}","repository_ids":["this-repo"],"primary_repository_id":"this-repo","scope":{scope}}}"#
    )
}

/// Plants a task through an open store handle.
pub fn plant(store: &dc_store::Store, id: &str, status: &str, scope: &str) {
    store.ensure_local_repository().expect("local repository");
    store
        .workbench_request("items.put", &item_request(id, status, scope))
        .unwrap_or_else(|e| panic!("planting {id}: {e}"));
}

/// Plants a task the way an operator does, through the `dcstore` binary.
pub fn plant_db(db: &Path, id: &str, status: &str, scope: &str) {
    let repo = dcstore(db, &["local-repository"]);
    assert_eq!(repo.code, 0, "local repository: {}", repo.stdout);
    let reply = dcstore(
        db,
        &[
            "work",
            "--method",
            "items.put",
            "--input",
            &item_request(id, status, scope),
        ],
    );
    assert!(
        reply.stdout.contains("\"ok\":true"),
        "planting {id}: {}",
        reply.stdout
    );
}
