//! The path a root-taking subcommand names must be a directory that exists.
//!
//! Measured through the release binary against a store built elsewhere:
//!
//! ```text
//! devmap --db <store> manifest    <missing>   exit 0, and created <missing>/.devmap/
//!                                             with the *other* repository's artifacts in it
//! devmap --db <store> routes      <missing>   exit 0, answered from the store's recorded root
//! devmap --db <store> shape-check <missing>   exit 0, same
//! devmap --db <store> api-impact  <missing>   exit 0, same
//! devmap --db <store> build       <file>      exit 0, an empty generation (fixed at the walker)
//! devmap --db <store> build       <missing>   exit 1
//! ```
//!
//! A path the caller named and this binary could not examine is not a
//! repository root, and answering — or worse, writing — as if it were is the
//! check that could not run reporting as one that ran. The default `.` always
//! exists, so only a path the caller actually spelled can fail this.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-root-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every subcommand whose positional is a repository root, with the
/// arguments that precede it.
const ROOT_TAKING: &[&[&str]] = &[
    &["manifest"],
    &["freshness"],
    &["routes"],
    &["shape-check"],
    &["api-impact", "GET /x"],
    &["build", "--progress", "never"],
];

fn run(cwd: &Path, db: &Path, args: &[&str], root: &Path) -> (Option<i32>, String, String) {
    let out = Command::new(devmap())
        .arg("--json")
        .arg("--db")
        .arg(db)
        .args(args)
        .arg(root)
        .current_dir(cwd)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut diagnostic = String::from_utf8_lossy(&out.stderr).into_owned();
    if diagnostic.is_empty() {
        // Progress is bounded: a stalled renderer reports its retained error
        // in JSON instead of holding the command open indefinitely.
        let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(payload["progress_output"]["incomplete"], true);
        let retained = payload["progress_output"]["diagnostics"]["unrendered"]
            .as_array()
            .unwrap();
        assert!(
            !retained.is_empty(),
            "a failed renderer must preserve the refusal"
        );
        diagnostic = retained
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
    }
    (out.status.code(), stdout, diagnostic)
}

#[test]
fn a_root_that_does_not_exist_is_refused_by_name_and_nothing_is_written_there() {
    let dir = scratch("missing");
    let db = dir.join("devmap.sqlite");
    let missing = dir.join("nowhere");
    for args in ROOT_TAKING {
        let (code, stdout, stderr) = run(&dir, &db, args, &missing);
        assert_ne!(
            code,
            Some(0),
            "{args:?} {}: a root that does not exist must be refused: {stdout}",
            missing.display()
        );
        assert!(
            stderr.contains("nowhere"),
            "{args:?}: the refusal must name the root: {stderr}"
        );
        assert!(
            stdout.trim().starts_with('{') && stdout.contains("\"error\""),
            "{args:?}: under --json the refusal is one JSON line on stdout: {stdout:?}"
        );
        assert!(
            !missing.exists(),
            "{args:?}: nothing may be created under a root that did not exist"
        );
    }
}

#[test]
fn a_root_that_is_a_regular_file_is_refused_as_not_a_directory() {
    let dir = scratch("file");
    let db = dir.join("devmap.sqlite");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "not a repository\n").unwrap();
    for args in ROOT_TAKING {
        let (code, stdout, stderr) = run(&dir, &db, args, &file);
        assert_ne!(
            code,
            Some(0),
            "{args:?}: a root that is a file must be refused: {stdout}"
        );
        assert!(
            stderr.contains("notes.txt") && stderr.contains("not a directory"),
            "{args:?}: the refusal must name the root and say why: {stderr}"
        );
    }
}

/// The check is about the named root, not about the store: the default `.`
/// from a directory that exists still reaches the subcommand.
#[test]
fn the_default_root_is_untouched_by_the_check() {
    let dir = scratch("default");
    let db = dir.join("devmap.sqlite");
    let out = Command::new(devmap())
        .args(["--json", "--db"])
        .arg(&db)
        .arg("freshness")
        .current_dir(&dir)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "freshness of the working directory must still answer: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !stdout.contains("\"error\""),
        "an existing default root is not a refusal: {stdout}"
    );
}
