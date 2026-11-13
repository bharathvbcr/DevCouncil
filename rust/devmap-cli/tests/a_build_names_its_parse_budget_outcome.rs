//! A build says what the parse budget did to it. The counts already lived under
//! `file_progress.extraction.budget_retry`; a caller that wants to know whether
//! the generation it just committed is complete should not have to know that
//! nesting, and a healthy build must not alarm.
//!
//! Forcing a real refusal through the binary needs a file that overruns both
//! the 5 s and the 25 s budget, which is not a deterministic fixture; the
//! rendering of a non-zero count is pinned by the unit tests beside
//! `parse_budget_diagnostic`, and the retry itself by the devmap-extract
//! `budget_retry` tests. This test pins the part only the binary can show: the
//! key is on the result of a build that extracted, equals the nested counts,
//! and a clean build prints no `parse budget:` line.

use std::path::PathBuf;
use std::process::Command;

fn devmap() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devmap"))
}

#[test]
fn a_clean_build_reports_zero_refusals_and_prints_no_diagnostic() {
    let root = std::env::temp_dir().join(format!("devmap-parse-budget-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(
        root.join("lib.py"),
        "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n",
    )
    .unwrap();
    let db = root.join("index.sqlite");

    let out = Command::new(devmap())
        .arg("--db")
        .arg(&db)
        .arg("--json")
        .args(["build", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let result: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("build not JSON ({e}): {stdout}\n{stderr}"));

    let key = result
        .get("parse_budget")
        .unwrap_or_else(|| panic!("key parse_budget absent from a build that extracted: {result}"));
    assert_eq!(key["still_refused"], 0, "{key}");
    assert_eq!(key["not_retried"], 0, "{key}");
    assert_eq!(
        key, &result["file_progress"]["extraction"]["budget_retry"],
        "the top-level key must be the nested counts, not a second source"
    );
    assert!(
        !stderr.contains("parse budget:"),
        "a healthy build must not alarm: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
