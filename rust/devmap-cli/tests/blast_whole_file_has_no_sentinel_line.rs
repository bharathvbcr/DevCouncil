//! A whole-file `blast --at <bare path>` target is a range with no end line.
//!
//! `dc_regress::change::parse_location` spells "the whole file" as
//! `1..=u32::MAX`, and that sentinel used to reach the JSON as a real line
//! number (`end_line: 4294967295`). The `--json` output is read by GitPulse
//! and MCP clients, so the end line is now `null` with `whole_file: true`
//! beside it, and no blast or suspects field may carry the sentinel.

use std::path::{Path, PathBuf};
use std::process::Command;

const SENTINEL: u64 = u32::MAX as u64;

const BASE: &str = "\
pub fn helper(flag: bool) -> bool {
    flag
}

pub fn caller() -> bool {
    helper(true)
}
";

const CHANGED: &str = "\
pub fn helper(flag: bool) -> bool {
    !flag
}

pub fn caller() -> bool {
    helper(true)
}
";

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

fn devmap_json(repo: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--json")
        .args(args)
        .arg("--root")
        .arg(repo)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{args:?} failed: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{args:?} must emit JSON: {e}\n{stdout}"))
}

fn setup(label: &str) -> (PathBuf, String) {
    let repo = std::env::temp_dir().join(format!(
        "devmap-blast-whole-file-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("src")).expect("temp root");
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Ada"]);
    git(&repo, &["config", "user.email", "ada@example.com"]);
    std::fs::write(repo.join("src/lib.rs"), BASE).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "base", "--no-gpg-sign"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("src/lib.rs"), CHANGED).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "change", "--no-gpg-sign"]);
    let built = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["build", "--full", "--root"])
        .arg(&repo)
        .output()
        .expect("build runs");
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stdout)
    );
    (repo, base)
}

/// Path of the first JSON number equal to the sentinel, if any.
fn find_sentinel(value: &serde_json::Value, path: &str) -> Option<String> {
    match value {
        serde_json::Value::Number(n) if n.as_u64() == Some(SENTINEL) => Some(path.to_string()),
        serde_json::Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, v)| find_sentinel(v, &format!("{path}[{i}]"))),
        serde_json::Value::Object(map) => map
            .iter()
            .find_map(|(k, v)| find_sentinel(v, &format!("{path}.{k}"))),
        _ => None,
    }
}

#[test]
fn a_bare_path_target_reports_a_whole_file_range_without_a_sentinel() {
    let (repo, _base) = setup("bare");
    let report = devmap_json(&repo, &["blast", "--at", "src/lib.rs"]);

    let range = &report["changed_files"][0]["ranges"][0];
    assert_eq!(range["start_line"], 1, "{report:#}");
    assert!(
        range["end_line"].is_null(),
        "a whole-file range has no end line: {range}"
    );
    assert_eq!(range["whole_file"], true, "{range}");
    assert_eq!(find_sentinel(&report, "$"), None, "{report:#}");
}

#[test]
fn an_explicit_line_range_keeps_its_end_line_and_is_not_whole_file() {
    let (repo, _base) = setup("range");
    let report = devmap_json(&repo, &["blast", "--at", "src/lib.rs:2-3"]);

    let range = &report["changed_files"][0]["ranges"][0];
    assert_eq!(range["start_line"], 2, "{report:#}");
    assert_eq!(range["end_line"], 3, "{report:#}");
    assert!(range.get("whole_file").is_none(), "{range}");
}

#[test]
fn no_blast_or_suspects_field_emits_the_sentinel() {
    let (repo, base) = setup("all");
    for (label, args) in [
        ("blast --at bare", vec!["blast", "--at", "src/lib.rs"]),
        ("blast --since", vec!["blast", "--since", base.as_str()]),
        (
            "suspects --since",
            vec!["suspects", "caller", "--since", base.as_str()],
        ),
    ] {
        let report = devmap_json(&repo, &args);
        assert_eq!(find_sentinel(&report, "$"), None, "{label}: {report:#}");
    }
}
