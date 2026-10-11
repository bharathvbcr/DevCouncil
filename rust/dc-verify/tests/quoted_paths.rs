//! Paths in real `git diff` output.
//!
//! Git C-quotes a name holding a control character, a quote, a backslash or a
//! non-ASCII byte, and does not quote a name that merely holds spaces. The
//! parser trimmed the quotes and kept the escapes, so `café.go` became the
//! orphan `caf/303/251.go`, and it split `diff --git` headers on the last
//! ` b/`, so a deleted `x b/secret.go` was judged as `secret.go`. These tests
//! read what git actually prints, not a hand-written imitation of it.

use std::path::Path;
use std::process::Command;

use dc_verify::{ChangeStatus, classify_scope, parse_unified};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e.invalid")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dcv-quoted-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    dir
}

fn write(dir: &Path, rel: &str, body: &str) {
    let full = dir.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
}

#[test]
fn quoted_names_from_real_git_output_are_decoded() {
    let dir = repo("quoted");
    write(&dir, "dir b/gone.go", "package gone\n");
    write(&dir, "x b/secret.go", "package secret\n");
    write(&dir, "old name.go", "package old\nfunc a() {}\n");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "base"]);

    write(&dir, "café.go", "package cafe\n");
    write(&dir, "tab\tname.go", "package tab\n");
    std::fs::remove_file(dir.join("dir b/gone.go")).unwrap();
    std::fs::remove_file(dir.join("x b/secret.go")).unwrap();
    git(&dir, &["mv", "old name.go", "née.go"]);
    git(&dir, &["add", "-A"]);
    // Default config on purpose: this is the quoted spelling the Go side had
    // no way to avoid before it passed core.quotePath=false.
    let diff = git(&dir, &["diff", "--cached", "-M", "HEAD"]);
    assert!(
        diff.contains("\\303\\251"),
        "fixture: git did not quote: {diff}"
    );

    let files = parse_unified(&diff).expect("real git output parses");
    let mut paths: Vec<(&str, ChangeStatus)> =
        files.iter().map(|f| (f.path.as_str(), f.status)).collect();
    paths.sort_by_key(|(p, _)| *p);
    assert_eq!(
        paths,
        vec![
            ("café.go", ChangeStatus::Added),
            ("dir b/gone.go", ChangeStatus::Deleted),
            ("née.go", ChangeStatus::Renamed),
            ("tab\tname.go", ChangeStatus::Added),
            ("x b/secret.go", ChangeStatus::Deleted),
        ]
    );
    let renamed = files.iter().find(|f| f.path == "née.go").unwrap();
    assert_eq!(renamed.old_path.as_deref(), Some("old name.go"));

    // Planned exactly as the files are named, nothing is an orphan.
    let planned: Vec<String> = [
        "café.go",
        "tab\tname.go",
        "dir b/gone.go",
        "x b/secret.go",
        "née.go",
        "old name.go",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let report = classify_scope(&files, &planned);
    assert!(report.orphans.is_empty(), "orphans: {:?}", report.orphans);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_deleted_file_with_b_slash_in_its_name_is_not_read_as_a_shorter_path() {
    let dir = repo("bslash");
    write(&dir, "x b/secret.go", "package secret\n");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "base"]);
    std::fs::remove_file(dir.join("x b/secret.go")).unwrap();
    let diff = git(&dir, &["diff", "HEAD"]);
    let files = parse_unified(&diff).expect("parses");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "x b/secret.go", "header: {diff}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_diff_git_never_produces_is_refused() {
    let deleted_with_additions = "diff --git a/f.go b/f.go\ndeleted file mode 100644\n--- a/f.go\n+++ /dev/null\n@@ -1,1 +1,1 @@\n-old\n+new\n";
    let err = parse_unified(deleted_with_additions).unwrap_err();
    assert!(err.reason.contains("deleted file"), "{err}");

    let one_path_twice = "diff --git a/f.go b/f.go\n--- a/f.go\n+++ b/f.go\n@@ -1,1 +1,1 @@\n-a\n+b\ndiff --git a/f.go b/f.go\n--- a/f.go\n+++ b/f.go\n@@ -3,1 +3,1 @@\n-c\n+d\n";
    let err = parse_unified(one_path_twice).unwrap_err();
    assert!(err.reason.contains("more than once"), "{err}");

    let overlapping = "diff --git a/f.go b/f.go\n--- a/f.go\n+++ b/f.go\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -2,2 +2,2 @@\n-b\n+X\n c\n";
    let err = parse_unified(overlapping).unwrap_err();
    assert!(err.reason.contains("overlaps"), "{err}");

    // Honest diffs of the same shapes still parse.
    let apart = "diff --git a/f.go b/f.go\n--- a/f.go\n+++ b/f.go\n@@ -1,1 +1,1 @@\n-a\n+b\n@@ -9,1 +9,1 @@\n-c\n+d\n";
    assert_eq!(
        parse_unified(apart).expect("apart hunks are fine")[0]
            .added_lines
            .len(),
        2
    );
}
