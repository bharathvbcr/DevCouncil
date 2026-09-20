//! `devmap suspects` end to end: build an index over a real repository, then
//! ask which commit could have caused a symptom.
//!
//! The library half is tested in `dc-regress`. What this proves is the wiring
//! nothing else can: that the store adapter hands over spans whose basis lines
//! up with the blob the analysis blames, that the cone comes out of the real
//! blast-radius walk, and that the command reports a refusal as a refusal.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

fn temp_root(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "devmap-suspects-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("temp root");
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
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn devmap(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(args)
        .arg("--root")
        .arg(repo)
        .output()
        .expect("devmap runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const GOOD: &str = "\
pub fn helper(flag: bool) -> bool {
    flag
}

pub fn caller() -> bool {
    helper(true)
}
";

const BUGGED: &str = "\
pub fn helper(flag: bool) -> bool {
    !flag
}

pub fn caller() -> bool {
    helper(true)
}
";

/// An unrelated later commit, so the window holds more than the bug.
const NOISE: &str = "\
pub fn helper(flag: bool) -> bool {
    !flag
}

pub fn caller() -> bool {
    helper(true)
}

pub fn unrelated() -> u32 {
    7
}
";

fn setup(label: &str) -> (PathBuf, String, String) {
    let repo = temp_root(label);
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Ada"]);
    git(&repo, &["config", "user.email", "ada@example.com"]);

    std::fs::write(repo.join("src/lib.rs"), GOOD).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "good", "--no-gpg-sign"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);

    std::fs::write(repo.join("src/lib.rs"), BUGGED).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "bug", "--no-gpg-sign"]);
    let bug = git(&repo, &["rev-parse", "HEAD"]);

    std::fs::write(repo.join("src/lib.rs"), NOISE).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "noise", "--no-gpg-sign"]);

    let (ok, out, err) = devmap(&repo, &["build", "--full"]);
    assert!(ok, "build failed: {out}{err}");
    (repo, base, bug)
}

#[test]
fn suspects_names_the_commit_that_changed_the_cone() {
    let (repo, base, bug) = setup("names");
    let (ok, out, err) = devmap(&repo, &["--json", "suspects", "caller", "--since", &base]);
    assert!(ok, "suspects failed: {out}{err}");

    let report: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("suspects must emit JSON: {e}\n{out}"));
    let suspects = report["suspects"].as_array().expect("a suspects array");
    assert!(
        !suspects.is_empty(),
        "the commit that changed `helper` is in the cone of `caller`: {report:#}"
    );
    let named: Vec<&str> = suspects
        .iter()
        .filter_map(|s| s["commit"].as_str())
        .collect();
    assert!(
        named.contains(&bug.as_str()),
        "the bug commit {bug} must be among the suspects; got {named:?}\n{report:#}"
    );

    // The human-readable branch, which the JSON path does not exercise at all.
    // It rendered a refusal correctly on the first manual run and its success
    // branch had never been run by anything.
    let (ok, text, err) = devmap(&repo, &["suspects", "caller", "--since", &base]);
    assert!(ok, "the text rendering must not fail: {text}{err}");
    let short: String = bug.chars().take(12).collect();
    assert!(
        text.contains(&short),
        "the rendered report names the suspect commit; got:\n{text}"
    );
    assert!(
        text.contains("suspect(s) over"),
        "the header states how much was examined; got:\n{text}"
    );
}

/// The honesty property, through the whole stack: a report always says whether
/// it is complete, and an empty list plus `complete: true` is a finding rather
/// than a shrug.
#[test]
fn a_report_always_states_whether_it_is_complete() {
    let (repo, _base, _bug) = setup("complete");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    // An empty window: nothing is reachable from HEAD but not from HEAD.
    let (ok, out, err) = devmap(&repo, &["--json", "suspects", "caller", "--since", &head]);
    assert!(ok, "suspects failed: {out}{err}");
    let report: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert!(
        report["complete"].is_boolean(),
        "every report carries the flag: {report:#}"
    );
    assert_eq!(
        report["suspects"].as_array().map(Vec::len),
        Some(0),
        "an empty window holds no commits"
    );
}

/// A symptom the index does not know must be refused by name, not answered
/// with an empty list.
#[test]
fn an_unknown_symptom_is_refused_rather_than_answered_empty() {
    let (repo, base, _bug) = setup("unknown");
    let (ok, out, err) = devmap(
        &repo,
        &[
            "--json",
            "suspects",
            "no_such_symbol_anywhere",
            "--since",
            &base,
        ],
    );
    assert!(ok, "suspects failed: {out}{err}");
    let report: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(
        report["complete"].as_bool(),
        Some(false),
        "an unresolvable symptom is not a clean answer: {report:#}"
    );
    let kinds: Vec<&str> = report["unavailable"]
        .as_array()
        .expect("unavailable array")
        .iter()
        .filter_map(|u| u["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"symptom_not_found"),
        "the refusal names what went wrong; got {kinds:?}"
    );
}

/// `--since` is the bound that makes this a window rather than a history walk.
#[test]
fn an_empty_since_is_refused_before_any_work() {
    let (repo, _base, _bug) = setup("empty-since");
    let (ok, _out, err) = devmap(&repo, &["suspects", "caller", "--since", "   "]);
    assert!(!ok, "a blank --since must be refused");
    assert!(
        err.contains("--since must name a revision"),
        "the refusal explains why; got {err}"
    );
}
