//! Lane `verify-residue-2`: a changed source the tree never read is reported.
//!
//! `read_post_image` used to fold five causes into one miss that the caller
//! mapped to "no source", and a source that disagreed with the diff was just as
//! silent: the file took the substring checks with no `stub_ast_degraded` entry,
//! which reads like a file the tree analysed. Each cause now has its own reason,
//! and a file that has nothing to read (a deletion, no added lines, a path the
//! tree does not parse) stays undegraded.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use dc_verify::parse_unified;
use dc_verify::rigor::detect_stubs_report;

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dcv-unreadable-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("src")).unwrap();
        Self(base)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        // A test may leave a file it made unreadable.
        let _ = std::fs::set_permissions(
            self.0.join("src/a.rs"),
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(root: Option<&Path>, diff: &str) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dcverify"));
    cmd.arg("check");
    if let Some(root) = root {
        cmd.arg("--root").arg(root);
    }
    let mut child = cmd
        .args(["--planned", "src/a.rs"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let body = diff.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(body.as_bytes());
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A modification that adds one line at line 3.
const MODIFY: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub fn f() -> u32 { 1 }\n";
const GOOD_SOURCE: &str = "// pad\n// pad\npub fn f() -> u32 { 1 }\n";

fn degraded_entry(out: &str, reason: &str) -> bool {
    out.contains(&format!("\"src/a.rs: {reason}\""))
}

fn degraded_at_all(out: &str) -> bool {
    out.contains("stub_ast_degraded\":[\"")
}

fn assert_degraded(out: &str, reason: &str) {
    assert!(out.contains("\"ok\":true"), "did not run: {out}");
    assert!(
        degraded_entry(out, reason),
        "no `src/a.rs: {reason}` entry: {}",
        out.split("stub_ast_degraded").nth(1).unwrap_or("-")
    );
}

fn assert_not_degraded(out: &str) {
    assert!(out.contains("\"ok\":true"), "did not run: {out}");
    assert!(
        !degraded_at_all(out),
        "a file with nothing to read was degraded: {}",
        out.split("stub_ast_degraded").nth(1).unwrap_or("-")
    );
}

#[test]
fn control_a_readable_matching_source_is_not_degraded() {
    let root = Root::new();
    std::fs::write(root.0.join("src/a.rs"), GOOD_SOURCE).unwrap();
    assert_not_degraded(&check(Some(&root.0), MODIFY));
}

#[test]
fn no_root_is_degraded() {
    assert_degraded(
        &check(None, MODIFY),
        "source not read: no working-tree root",
    );
}

#[test]
fn a_symlinked_source_is_degraded_as_outside_the_tree() {
    let root = Root::new();
    let elsewhere = Root::new();
    std::fs::write(elsewhere.0.join("src/a.rs"), GOOD_SOURCE).unwrap();
    std::os::unix::fs::symlink(elsewhere.0.join("src/a.rs"), root.0.join("src/a.rs")).unwrap();
    assert_degraded(
        &check(Some(&root.0), MODIFY),
        "source not read: path outside the working tree",
    );
}

#[test]
fn a_symlinked_parent_directory_is_degraded_as_outside_the_tree() {
    let root = Root::new();
    let elsewhere = Root::new();
    std::fs::write(elsewhere.0.join("src/a.rs"), GOOD_SOURCE).unwrap();
    std::fs::remove_dir_all(root.0.join("src")).unwrap();
    std::os::unix::fs::symlink(elsewhere.0.join("src"), root.0.join("src")).unwrap();
    assert_degraded(
        &check(Some(&root.0), MODIFY),
        "source not read: path outside the working tree",
    );
}

#[test]
fn a_missing_source_is_degraded() {
    let root = Root::new();
    assert_degraded(
        &check(Some(&root.0), MODIFY),
        "source not read: file missing",
    );
}

#[test]
fn a_directory_where_the_source_should_be_is_degraded() {
    let root = Root::new();
    std::fs::create_dir(root.0.join("src/a.rs")).unwrap();
    assert_degraded(
        &check(Some(&root.0), MODIFY),
        "source not read: not a regular file",
    );
}

#[test]
fn a_fifo_where_the_source_should_be_is_degraded_without_being_opened() {
    let root = Root::new();
    let made = Command::new("mkfifo").arg(root.0.join("src/a.rs")).status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("mkfifo is not available; skipping");
        return;
    }
    // Opening a FIFO for reading blocks until a writer appears, so a reader
    // that opened it would hang here and the test runner would time out.
    assert_degraded(
        &check(Some(&root.0), MODIFY),
        "source not read: not a regular file",
    );
}

#[test]
fn an_unreadable_source_is_degraded() {
    let root = Root::new();
    let path = root.0.join("src/a.rs");
    std::fs::write(&path, GOOD_SOURCE).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        eprintln!("this user can read a mode-000 file; skipping");
        return;
    }
    assert_degraded(&check(Some(&root.0), MODIFY), "source not read: unreadable");
}

#[test]
fn a_source_that_disagrees_with_the_diff_is_degraded() {
    let root = Root::new();
    std::fs::write(
        root.0.join("src/a.rs"),
        "// pad\n// pad\npub fn g() -> u32 { 2 }\n",
    )
    .unwrap();
    assert_degraded(
        &check(Some(&root.0), MODIFY),
        "source does not match the diff",
    );
}

// ---- files with nothing to read stay undegraded ---------------------------------

#[test]
fn a_deleted_file_is_not_degraded() {
    let root = Root::new();
    let diff = "diff --git a/src/a.rs b/src/a.rs\ndeleted file mode 100644\n--- a/src/a.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-// pad\n-// pad\n";
    assert_not_degraded(&check(Some(&root.0), diff));
    assert_not_degraded(&check(None, diff));
}

#[test]
fn a_rename_only_is_not_degraded() {
    let root = Root::new();
    let diff = "diff --git a/src/a.rs b/src/b.rs\nsimilarity index 100%\nrename from src/a.rs\nrename to src/b.rs\n";
    assert_not_degraded(&check(Some(&root.0), diff));
}

#[test]
fn a_change_that_only_removes_lines_is_not_degraded() {
    let root = Root::new();
    let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,2 @@\n // pad\n-// gone\n // pad\n";
    assert_not_degraded(&check(Some(&root.0), diff));
}

#[test]
fn a_path_the_tree_does_not_parse_is_not_degraded() {
    let root = Root::new();
    let diff = "diff --git a/notes.txt b/notes.txt\n--- a/notes.txt\n+++ b/notes.txt\n@@ -1,2 +1,3 @@\n a\n b\n+c\n";
    assert_not_degraded(&check(Some(&root.0), diff));
    assert_not_degraded(&check(None, diff));
}

// ---- the library: a disagreeing source ----------------------------------------------

#[test]
fn library_a_disagreeing_source_is_unparsed_and_an_agreeing_one_is_not() {
    let files = parse_unified(MODIFY).unwrap();
    let agree = detect_stubs_report(&files, &|_| Some(GOOD_SOURCE.to_string()));
    assert!(agree.unparsed.is_empty(), "{:?}", agree.unparsed);
    let moved = detect_stubs_report(&files, &|_| {
        Some("// pad\n// pad\npub fn g() -> u32 { 2 }\n".to_string())
    });
    let reasons: Vec<&str> = moved.unparsed.iter().map(|u| u.reason).collect();
    assert_eq!(reasons, vec!["source does not match the diff"]);
    // The library cannot say why a source is absent; the binary does.
    let none = detect_stubs_report(&files, &|_| None);
    assert!(none.unparsed.is_empty(), "{:?}", none.unparsed);
}

#[test]
fn library_a_disagreeing_source_for_a_path_the_tree_does_not_parse_is_not_unparsed() {
    let diff = "diff --git a/notes.txt b/notes.txt\n--- a/notes.txt\n+++ b/notes.txt\n@@ -1,2 +1,3 @@\n a\n b\n+c\n";
    let files = parse_unified(diff).unwrap();
    let r = detect_stubs_report(&files, &|_| Some("x\ny\nz\n".to_string()));
    assert!(r.unparsed.is_empty(), "{:?}", r.unparsed);
}

// ---- no secret in a reason ----------------------------------------------------------

#[test]
fn a_degradation_reason_carries_the_path_and_never_a_line() {
    let root = Root::new();
    let key = concat!("AKIA", "IOSFODNN7EXAMPLE");
    let diff = format!(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub const K: &str = \"{key}\";\n"
    );
    let out = check(Some(&root.0), &diff);
    assert!(
        degraded_entry(&out, "source not read: file missing"),
        "{out}"
    );
    let degraded = out.split("stub_ast_degraded").nth(1).unwrap_or("");
    assert!(
        !degraded.contains(key),
        "a reason carried a line of the diff"
    );
}
