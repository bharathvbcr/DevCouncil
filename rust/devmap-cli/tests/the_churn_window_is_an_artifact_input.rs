//! The artifact stamp records the day the churn window is anchored to.
//!
//! `hotspots` is churn × fan-in, and churn is `git log --since=90.days` —
//! relative to *now*. Every other input the stamp compares is a property of
//! the tree (`built_head`, the fingerprints, the schema), so on a repository
//! with no new commit the artifacts matched every input day after day and
//! were kept while the window rolled its oldest commits out underneath them:
//! hotspot counts that only ever shrank, presented as computed. Lane G2 found
//! it and named the fix; this pins it.
//!
//! Day granularity, so the price is one regeneration per calendar day at
//! most, and only on a run that would otherwise have skipped.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

fn run_json(root: &Path, args: &[&str]) -> Value {
    let mut argv = vec!["--json"];
    argv.extend_from_slice(args);
    let output = run(root, &argv);
    assert!(
        output.status.success(),
        "devmap {argv:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next_back()
        .unwrap_or_else(|| panic!("devmap {argv:?} printed nothing on stdout"));
    serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("devmap {argv:?}: stdout is not JSON ({error}): {line}"))
}

/// A small git repository with one commit, so churn has history to read.
fn fixture(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "devmap-churn-day-{name}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/a.py"),
        "def helper():\n    return 1\n\n\ndef caller():\n    return helper()\n",
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "fixture"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    root
}

fn db(root: &Path) -> String {
    devmap_extract::paths::store_path(root)
        .to_string_lossy()
        .into_owned()
}

fn stamp_path(db: &str) -> PathBuf {
    PathBuf::from(format!("{db}.artifacts.json"))
}

fn read_stamp(db: &str) -> Value {
    let text = std::fs::read_to_string(stamp_path(db)).expect("the stamp exists after a write");
    serde_json::from_str(&text).expect("the stamp is JSON")
}

fn recorded_day(stamp: &Value) -> u64 {
    stamp["inputs"]["churn_window_day"]
        .as_u64()
        .unwrap_or_else(|| {
            panic!(
                "the stamp does not record the day the churn window is anchored to; \
                 its inputs are {}",
                stamp["inputs"]
            )
        })
}

#[test]
fn the_stamp_records_the_day_the_churn_window_is_anchored_to() {
    let root = fixture("records");
    let db = db(&root);
    run_json(&root, &["--db", &db, "build", ".", "--manifest", "--force"]);

    let recorded = recorded_day(&read_stamp(&db));
    let today = devmap_query::inventory::churn_window_day();
    // A midnight between the write and this read is one day, not a failure.
    assert!(
        recorded.abs_diff(today) <= 1,
        "the recorded day must be the writer's today: recorded {recorded}, today {today}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_new_day_defeats_the_skip_and_the_same_day_keeps_it() {
    let root = fixture("rolls");
    let db = db(&root);
    let base = ["--db", &db, "build", ".", "--manifest", "--force"];
    run_json(&root, &base);
    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        json!(true),
        "baseline: the second run on the same day must skip"
    );

    // Roll the stamp back one day: the artifacts on disk are now what
    // yesterday's window produced, and today's window is a different set of
    // commits.
    let mut stamp = read_stamp(&db);
    let yesterday = recorded_day(&stamp) - 1;
    stamp["inputs"]["churn_window_day"] = json!(yesterday);
    std::fs::write(stamp_path(&db), serde_json::to_vec(&stamp).unwrap()).unwrap();

    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        json!(false),
        "artifacts written under yesterday's churn window must be regenerated today"
    );
    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        json!(true),
        "and once regenerated they skip again for the rest of the day"
    );
    let _ = std::fs::remove_dir_all(root);
}
