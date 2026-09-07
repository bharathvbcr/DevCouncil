//! `devmap paths` reports where a repository's state lives without opening it.
//!
//! The Python seam's state-directory resolver asked `devmap --json status` for
//! `db_path`, once per process. `status` opens the store to count nodes, so the
//! question "where is the store" cost 29 ms against this repository's 168 MB
//! one — and could not be answered at all for a store `status` cannot open. The
//! kernel resolves every path before it opens anything (`devmap_extract::paths`);
//! `paths` reports exactly that, and nothing else.

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
        "devmap-paths-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn paths(
    root: &Path,
    extra: &[&str],
    env: &[(&str, &str)],
) -> (Option<i32>, serde_json::Value, String) {
    let mut command = Command::new(devmap());
    command
        .arg("--json")
        .args(extra)
        .arg("paths")
        .arg(root)
        .current_dir(root);
    command.env_remove("DEVMAP_HOME");
    for (key, value) in env {
        command.env(key, value);
    }
    let out = command.output().expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    (
        out.status.code(),
        serde_json::from_str(stdout.trim()).unwrap_or(serde_json::Value::Null),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn paths_reports_the_resolved_layout_and_never_opens_the_store() {
    let root = scratch("layout");
    std::fs::create_dir_all(root.join(".devmap").join("codeintel")).unwrap();
    // A store `status` cannot open: `paths` must still answer.
    std::fs::write(
        root.join(".devmap/codeintel/devmap.sqlite"),
        b"this is not a database",
    )
    .unwrap();

    let (code, value, stderr) = paths(&root, &[], &[]);
    assert_eq!(code, Some(0), "{value}\n{stderr}");
    let state_dir = PathBuf::from(value["state_dir"].as_str().expect("state_dir"));
    assert_eq!(state_dir, root.join(".devmap"), "{value}");
    assert_eq!(
        value["db_path"].as_str(),
        Some(
            root.join(".devmap/codeintel/devmap.sqlite")
                .to_str()
                .unwrap()
        ),
        "{value}"
    );
    for (field, suffix) in [
        ("repo_map", ".devmap/repo_map.json"),
        ("code_graph", ".devmap/graph/code_graph.json"),
        ("workspace", ".devmap/workspace.json"),
    ] {
        assert_eq!(
            value[field].as_str(),
            Some(root.join(suffix).to_str().unwrap()),
            "{field}: {value}"
        );
    }
    assert_eq!(
        value["root"].as_str(),
        Some(root.to_str().unwrap()),
        "{value}"
    );
    assert!(
        value["state_dir_exists"] == true && value["store_exists"] == true,
        "existence is reported, not inferred: {value}"
    );
    // The garbage store is untouched: nothing was opened or migrated.
    assert_eq!(
        std::fs::read(root.join(".devmap/codeintel/devmap.sqlite")).unwrap(),
        b"this is not a database"
    );
}

#[test]
fn paths_honours_an_explicit_db_and_the_home_override() {
    let root = scratch("overrides");
    let explicit = root.join("elsewhere").join("devmap.sqlite");
    let (code, value, stderr) = paths(&root, &["--db", explicit.to_str().unwrap()], &[]);
    assert_eq!(code, Some(0), "{value}\n{stderr}");
    assert_eq!(value["db_path"].as_str(), explicit.to_str(), "{value}");

    let home = scratch("home");
    let (code, value, stderr) = paths(&root, &[], &[("DEVMAP_HOME", home.to_str().unwrap())]);
    assert_eq!(code, Some(0), "{value}\n{stderr}");
    assert_eq!(value["state_dir"].as_str(), home.to_str(), "{value}");
    assert_eq!(value["store_exists"], false, "{value}");
}

#[test]
fn a_root_that_does_not_exist_is_refused_like_every_other_named_root() {
    let dir = scratch("missing");
    let missing = dir.join("nowhere");
    let out = Command::new(devmap())
        .args(["--json", "paths"])
        .arg(&missing)
        .current_dir(&dir)
        .output()
        .expect("devmap runs");
    assert_ne!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nowhere"));
}
