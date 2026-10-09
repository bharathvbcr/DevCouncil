//! Two answers that read as complete checks and were not (merged audit item,
//! 2026-10-09, from the Manvi board).
//!
//! 1. Manvi's `crates/dc-*` are symlinks to `../../DevCouncil/rust/*`. The
//!    store held none of their files, `status` reported
//!    `discovery_refused.total = 0`, and `search detect_stubs` answered
//!    `Available, total: 0` for a function that exists under one of them.
//! 2. `neighbors seed_for_repeat` — an indexed function that calls nothing —
//!    answered its callees `Unavailable: … has no indexed traversal start`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "devmap-outside-index-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base.canonicalize().unwrap()
}

#[cfg(unix)]
#[test]
fn an_out_of_root_directory_link_is_refused_and_lookups_under_it_say_outside_index() {
    let base = scratch("dir-link");
    let outside = base.join("DevCouncil/rust/dc-verify/src");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        outside.join("rigor.py"),
        "def detect_stubs():\n    return []\n",
    )
    .unwrap();
    let root = base.join("Manvi");
    std::fs::create_dir_all(root.join("crates")).unwrap();
    std::fs::write(root.join("crates/README.md"), "crates\n").unwrap();
    std::fs::write(root.join("main.py"), "def main():\n    return 0\n").unwrap();
    std::os::unix::fs::symlink(
        "../../DevCouncil/rust/dc-verify",
        root.join("crates/dc-verify"),
    )
    .unwrap();

    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(
        build["discovery_refused_files"], 1,
        "the link out of the tree is a refusal: {build}"
    );

    let status = json(&root, &["--json", "status"]);
    let refused: Vec<&str> = status["coverage_gaps"]["discovery_refused"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["path"].as_str())
        .collect();
    assert_eq!(
        refused,
        vec!["crates/dc-verify"],
        "status lists the refused directory link: {status}"
    );
    assert_eq!(status["is_fresh"], true, "{status}");

    let search = json(&root, &["--json", "search", "detect_stubs"]);
    let reason = search["resolution"]["Unavailable"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("an empty search over a hole is not Available: {search}"));
    assert!(
        reason.starts_with("outside index") && reason.contains("crates/dc-verify"),
        "and names where the symbol may be: {reason}"
    );

    let explore = json(&root, &["--json", "explore", "detect_stubs"]);
    let reason = explore["definitions"]["resolution"]["Unavailable"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("explore answers the same way: {explore}"));
    assert!(reason.starts_with("outside index"), "{reason}");

    // A symbol that is indexed is still found, Available.
    let found = json(&root, &["--json", "search", "main"]);
    assert_eq!(found["resolution"], "Available", "{found}");
    assert_eq!(found["items"][0]["symbol_name"], "main", "{found}");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_leaf_functions_callees_are_an_empty_available_answer() {
    let root = scratch("leaf");
    std::fs::write(
        root.join("run.py"),
        "def seed_for_repeat(i):\n    return i * 7\n\n\ndef main():\n    return seed_for_repeat(1)\n",
    )
    .unwrap();
    let _ = json(&root, &["--json", "build", "."]);
    let neighbors = json(&root, &["--json", "neighbors", "seed_for_repeat"]);
    let entry = &neighbors["neighbors"][0];
    assert_eq!(
        entry["callees"]["resolution"], "Available",
        "a function that calls nothing has no callees, which is an answer: {neighbors}"
    );
    assert_eq!(entry["callees"]["total"], 0, "{neighbors}");
    assert_eq!(entry["callers"]["resolution"], "Available", "{neighbors}");
    assert!(
        entry["callers"]["total"].as_u64().unwrap_or(0) >= 1,
        "main calls it: {neighbors}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
