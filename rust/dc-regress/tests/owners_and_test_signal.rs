//! Adversarial cases for blast owners and per-module test signals.
//!
//! A missing git answer must not look like "no owners". An incomplete or
//! renamed-test walk must not look like "no tests".

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use dc_regress::blast::{
    blast_change_with_program, owners_for_paths_with_program, TestSignal, OWNER_COMMIT_CAP,
};
use dc_regress::change::{ChangeSet, ChangeStatus, ChangedRange, FileChange};
use dc_regress::{AffectedTestFile, BlobIdentity, CodeGraph, ConeEntry, GraphSymbol, Unavailable};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

fn temp_root(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "dc-regress-owners-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[derive(Default)]
struct FakeGraph {
    symbols: Vec<GraphSymbol>,
    basis: Option<BlobIdentity>,
    tests: Vec<AffectedTestFile>,
    tests_incomplete: bool,
}

impl CodeGraph for FakeGraph {
    fn symbols_in(&self, file: &str) -> (Vec<GraphSymbol>, BlobIdentity) {
        (
            self.symbols
                .iter()
                .filter(|s| s.file_path == file)
                .cloned()
                .collect(),
            self.basis.clone().unwrap_or(BlobIdentity::Unknown),
        )
    }
    fn cone(&self, _symbol: &str, _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn impacted(&self, _seeds: &[String], _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn affected_tests(&self, _seeds: &[String], _depth: u32) -> (Vec<AffectedTestFile>, bool) {
        (self.tests.clone(), self.tests_incomplete)
    }
    fn resolve_symptom(&self, _symptom: &str) -> Vec<String> {
        Vec::new()
    }
}

/// A repository with no commits at all. Owner lookup must refuse rather than
/// report an empty owners list as if nobody owned the paths.
#[test]
fn a_repo_with_no_commits_marks_owners_unavailable() {
    let root = temp_root("no-commits");
    git(&root, &["init", "--initial-branch=main"]);
    // No commit. `git log` on a path will be refused.
    let (owners, gaps) = owners_for_paths_with_program(
        std::ffi::OsStr::new("git"),
        &root,
        &["src/lib.rs".into()],
        OWNER_COMMIT_CAP,
        Duration::from_secs(5),
    );
    assert!(
        owners.is_empty(),
        "no commits means no authors to list: {owners:?}"
    );
    assert!(
        gaps.iter()
            .any(|g| matches!(g, Unavailable::OwnersUnavailable { .. })),
        "and the ledger must say why, not leave a silently empty owners list: {gaps:?}"
    );
}

/// An empty author is not invented into a placeholder owner. It is named as
/// unavailable so completeness stays honest.
#[test]
fn an_empty_author_is_unavailable_not_invented() {
    let root = temp_root("empty-author");
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    // Force an empty author on this commit. Git accepts it when the env vars
    // are set to empty strings and `--allow-empty-message` is not required for
    // a normal message.
    let out = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["commit", "-m", "empty-author", "--no-gpg-sign"])
        .env("GIT_AUTHOR_NAME", "")
        .env("GIT_AUTHOR_EMAIL", "")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("commit");
    if !out.status.success() {
        // Some git builds refuse an empty author outright. That refusal is
        // itself an unavailable owners answer when asked — exercise the
        // parser path with a synthetic log line instead.
        let (owners, gaps) = owners_for_paths_with_program(
            std::ffi::OsStr::new("git"),
            &root,
            &["src/lib.rs".into()],
            OWNER_COMMIT_CAP,
            Duration::from_secs(5),
        );
        let _ = (owners, gaps);
        // Fall through to the synthetic assertion below via a direct parse
        // of the empty-author shape the collector recognises.
    }

    // Drive the collector against a stub that emits one empty author line.
    let stub = root.join("empty-author-git");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(
            &stub,
            "#!/bin/sh\n# Emulate `git -C <repo> log …` with one empty author.\nif printf '%s' \"$*\" | grep -q 'log'; then\n  printf '\\0\\n'\n  exit 0\nfi\nexec git \"$@\"\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&stub).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        return;
    }

    let (owners, gaps) = owners_for_paths_with_program(
        stub.as_os_str(),
        &root,
        &["src/lib.rs".into()],
        OWNER_COMMIT_CAP,
        Duration::from_secs(5),
    );
    assert!(
        !owners
            .iter()
            .any(|o| o.name.is_empty() && o.email.is_empty()),
        "an empty author must not become an Owner row: {owners:?}"
    );
    assert!(
        gaps.iter().any(|g| matches!(
            g,
            Unavailable::OwnersUnavailable { reason } if reason.contains("empty author")
        )),
        "empty author must flip complete via unavailable: {gaps:?}"
    );
}

/// A timeout after the first of many paths keeps the owners already found and
/// records unavailable — it does not discard them or pretend the pass finished.
#[test]
fn a_timeout_after_one_path_keeps_partial_owners_and_is_incomplete() {
    let root = temp_root("timeout");
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn b() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);

    let counter = root.join("log-count");
    let stub = root.join("slow-second-git");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            "#!/bin/sh\nCOUNTER=\"{}\"\nis_log=0\nfor a in \"$@\"; do\n  [ \"$a\" = log ] && is_log=1\ndone\nif [ \"$is_log\" = 1 ]; then\n  n=$(cat \"$COUNTER\" 2>/dev/null || echo 0)\n  n=$((n+1))\n  echo \"$n\" > \"$COUNTER\"\n  if [ \"$n\" -ge 2 ]; then\n    sleep 30\n  fi\nfi\nexec git \"$@\"\n",
            counter.display()
        );
        std::fs::write(&stub, script).unwrap();
        let mut perms = std::fs::metadata(&stub).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        return;
    }

    let (owners, gaps) = owners_for_paths_with_program(
        stub.as_os_str(),
        &root,
        &["src/a.rs".into(), "src/b.rs".into()],
        OWNER_COMMIT_CAP,
        Duration::from_millis(400),
    );
    assert!(
        owners.iter().any(|o| o.email == "ada@example.com"),
        "the first path's owners must survive the later timeout: {owners:?}"
    );
    assert!(
        gaps.iter()
            .any(|g| matches!(g, Unavailable::OwnersUnavailable { .. })),
        "the timeout must be named, so complete stays false: {gaps:?}"
    );
}

/// End-to-end: a renamed test file with an empty reached-test list makes the
/// module signal `unavailable`, never silent `none`.
#[test]
fn a_renamed_test_file_is_unavailable_on_the_module_not_none() {
    let root = temp_root("rename-test");
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    let source = "pub fn helper() -> u32 {\n    1\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    git(&root, &["add", "-A"]);
    let head = {
        git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);
        git(&root, &["rev-parse", "HEAD"])
    };
    let start = source.find("pub fn helper").unwrap();
    let graph = FakeGraph {
        symbols: vec![GraphSymbol {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            span_start: start,
            span_end: source.len(),
            body_exact: None,
        }],
        basis: Some(BlobIdentity::Blob(git(
            &root,
            &["rev-parse", &format!("{head}:src/lib.rs")],
        ))),
        tests: Vec::new(),
        tests_incomplete: false,
    };
    let change = ChangeSet {
        files: vec![
            FileChange {
                path: "src/lib.rs".into(),
                renamed_from: None,
                status: ChangeStatus::Modified,
                ranges: vec![ChangedRange {
                    start_line: 2,
                    end_line: 2,
                    deletion_only: false,
                }],
            },
            FileChange {
                path: "tests/new.rs".into(),
                renamed_from: Some("tests/old.rs".into()),
                status: ChangeStatus::Modified,
                ranges: vec![],
            },
        ],
        capped: false,
    };
    let report = blast_change_with_program(
        std::ffi::OsStr::new("git"),
        &root,
        &graph,
        &change,
        "rename",
        &head,
        3,
    );
    let src = report
        .modules
        .iter()
        .find(|m| m.path == "src")
        .expect("src module");
    assert_eq!(
        src.test_signal,
        TestSignal::Unavailable,
        "a renamed test with no reached list must not read as none: {report:#?}"
    );
    assert_ne!(src.test_signal, TestSignal::None);
}

/// An incomplete affected-test walk sets `unavailable` on the module and on
/// the report — never `none`.
#[test]
fn an_incomplete_test_walk_is_unavailable_on_the_report() {
    let root = temp_root("tests-incomplete");
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    let source = "pub fn helper() -> u32 {\n    1\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let start = source.find("pub fn helper").unwrap();
    let graph = FakeGraph {
        symbols: vec![GraphSymbol {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            span_start: start,
            span_end: source.len(),
            body_exact: None,
        }],
        basis: Some(BlobIdentity::Blob(git(
            &root,
            &["rev-parse", &format!("{head}:src/lib.rs")],
        ))),
        tests: Vec::new(),
        tests_incomplete: true,
    };
    let change = ChangeSet {
        files: vec![FileChange {
            path: "src/lib.rs".into(),
            renamed_from: None,
            status: ChangeStatus::Modified,
            ranges: vec![ChangedRange {
                start_line: 2,
                end_line: 2,
                deletion_only: false,
            }],
        }],
        capped: false,
    };
    let report = blast_change_with_program(
        std::ffi::OsStr::new("git"),
        &root,
        &graph,
        &change,
        "incomplete",
        &head,
        3,
    );
    assert!(
        report
            .unavailable
            .iter()
            .any(|u| matches!(u, Unavailable::AffectedTestsIncomplete { .. })),
        "{report:#?}"
    );
    let src = report
        .modules
        .iter()
        .find(|m| m.path == "src")
        .expect("src");
    assert_eq!(src.test_signal, TestSignal::Unavailable);
    assert!(!report.complete);
}
