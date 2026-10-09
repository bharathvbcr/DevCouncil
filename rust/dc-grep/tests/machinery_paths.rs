//! A search *target* inside `.git` or `.devcouncil` is refused, through the
//! real binary, for both commands that take one.
//!
//! The walk excluded those names only below its target, so `path: ".git"`
//! walked `.git` from the inside: `.git/config` lines (remote URLs,
//! `http.extraheader`) came back from `search`, and `files` listed the object
//! store. Every spelling that canonicalises into either directory is tried,
//! next to an ordinary search of the same fixture that must still answer.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

mod support;

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-machinery-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fresh fixture");
        let repo = Self(root);
        repo.write(
            ".git/config",
            "[http]\n\textraheader = AUTHORIZATION: basic needle-git-secret\n",
        );
        repo.write(".git/objects/ab/cdef", "needle-git-object\n");
        repo.write(
            ".devcouncil/state.json",
            "{\"note\": \"needle-harness-state\"}\n",
        );
        repo.write("src/lib.rs", "// needle-ordinary\n");
        repo
    }

    fn write(&self, path: &str, contents: &str) {
        let full = self.0.join(path);
        fs::create_dir_all(full.parent().unwrap()).expect("fixture parent");
        fs::write(full, contents).expect("fixture file");
    }

    fn call(&self, command: &str, mut request: Value) -> (bool, Value) {
        request["root"] = json!(self.0);
        let (ok, stdout) =
            support::run(command, request.to_string().as_bytes(), support::CALL_BOUND);
        let reply: Value = serde_json::from_slice(&stdout).expect("one JSON reply");
        (ok, reply)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The reply carries nothing from inside the machinery: either a refusal, or
/// an answer with no path and no line from it.
fn assert_withheld(command: &str, path: &str, ok: bool, reply: &Value) {
    let text = reply.to_string();
    for leaked in [
        "needle-git-secret",
        "needle-git-object",
        "needle-harness-state",
        "objects/ab",
        "state.json",
        "config",
    ] {
        assert!(
            !text.contains(leaked),
            "{command} path={path:?} returned {leaked:?}: {reply}"
        );
    }
    assert!(
        !ok && reply["ok"] == json!(false),
        "{command} path={path:?} must refuse, not answer: {reply}"
    );
}

const MACHINERY_SPELLINGS: &[&str] = &[
    ".git",
    ".git/",
    "src/../.git",
    "./.git/objects",
    ".GIT",
    ".devcouncil",
    ".devcouncil/",
    "src/../.devcouncil",
];

#[test]
fn search_refuses_a_target_inside_git_or_devcouncil() {
    let repo = Repo::new();
    for path in MACHINERY_SPELLINGS {
        let (ok, reply) = repo.call(
            "search",
            json!({ "pattern": "needle", "path": path, "include_ignored": true }),
        );
        assert_withheld("search", path, ok, &reply);
    }
}

#[test]
fn files_refuses_a_target_inside_git_or_devcouncil() {
    let repo = Repo::new();
    for path in MACHINERY_SPELLINGS {
        let (ok, reply) = repo.call("files", json!({ "path": path, "include_ignored": true }));
        assert_withheld("files", path, ok, &reply);
    }
}

/// `rank` takes the same target through the same resolver. The index is built
/// first, so a refusal is about the target and not about a missing index.
#[test]
fn rank_refuses_a_target_inside_git_or_devcouncil() {
    let repo = Repo::new();
    let (ok, reply) = repo.call("index", json!({}));
    assert!(ok, "index must succeed: {reply}");
    for path in MACHINERY_SPELLINGS {
        let (ok, reply) = repo.call("rank", json!({ "query": "needle", "path": path }));
        assert_withheld("rank", path, ok, &reply);
    }
}

/// A worktree's `.git` is a file holding a gitdir pointer, and a target can
/// name it directly.
#[test]
fn a_worktree_git_file_is_refused_as_a_target() {
    let repo = Repo::new();
    fs::remove_dir_all(repo.0.join(".git")).expect("drop the directory");
    repo.write(".git", "gitdir: /elsewhere/needle-git-secret\n");
    for command in ["search", "files"] {
        let mut request = json!({ "path": ".git", "include_ignored": true });
        if command == "search" {
            request["pattern"] = json!("needle");
        }
        let (ok, reply) = repo.call(command, request);
        assert_withheld(command, ".git", ok, &reply);
    }
}

/// A symlink is resolved before it is judged, so an innocent name that points
/// into `.git` is refused like `.git` itself.
#[cfg(unix)]
#[test]
fn a_symlink_into_git_is_refused_as_a_target() {
    let repo = Repo::new();
    std::os::unix::fs::symlink(repo.0.join(".git"), repo.0.join("docs")).expect("symlink");
    let (ok, reply) = repo.call(
        "search",
        json!({ "pattern": "needle", "path": "docs", "include_ignored": true }),
    );
    assert_withheld("search", "docs", ok, &reply);
}

/// The control: the same fixture, searched the ordinary ways, answers as it
/// always did — the machinery excluded, everything else found.
#[test]
fn ordinary_targets_are_unchanged() {
    let repo = Repo::new();
    for path in ["", ".", "src", "src/lib.rs", "src/../src"] {
        let (ok, reply) = repo.call(
            "search",
            json!({ "pattern": "needle", "path": path, "include_ignored": true }),
        );
        assert!(ok, "search path={path:?} must succeed: {reply}");
        let text = reply.to_string();
        assert!(
            text.contains("needle-ordinary"),
            "search path={path:?}: {reply}"
        );
        assert!(
            !text.contains("needle-git"),
            "search path={path:?}: {reply}"
        );
        assert!(
            !text.contains("needle-harness"),
            "search path={path:?}: {reply}"
        );

        let (ok, reply) = repo.call("files", json!({ "path": path, "include_ignored": true }));
        assert!(ok, "files path={path:?} must succeed: {reply}");
        assert_eq!(reply["paths"], json!(["src/lib.rs"]), "files path={path:?}");
    }
}
