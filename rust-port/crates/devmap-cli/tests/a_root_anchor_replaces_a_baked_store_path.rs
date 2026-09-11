//! `--root` names the repository; the store is then discovered, not spelled.
//!
//! A host hook runs in the agent's current directory, which after a `cd` or a
//! worktree entry is not the repository the index belongs to. The old anchor was
//! `--db ${CLAUDE_PROJECT_DIR}/.devcouncil/codeintel/devmap.sqlite`, which
//! anchors correctly and freezes the state layout as it stood when the hook was
//! written: a repository keeping its state anywhere else gets a second, empty
//! store created beside it, and every query answers from the wrong one.
//!
//! `--root` resolves through the same `devmap_extract::paths` discovery every
//! other invocation uses, so a hook cannot disagree with the CLI about where a
//! repository's state lives.

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
        "devmap-rootarg-{tag}-{}-{}",
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

fn paths_json(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "devmap {args:?} in {}: {stdout}\n{}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{args:?}: {err}: {stdout}"))
}

/// The whole point: run from somewhere else, resolve the named repository.
#[test]
fn a_named_root_decides_the_store_from_another_directory() {
    let elsewhere = scratch("cwd");
    let project = scratch("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src").join("a.rs"), "fn a() {}\n").unwrap();

    let project_arg = project.to_string_lossy().into_owned();
    let anchored = paths_json(&elsewhere, &["--json", "--root", &project_arg, "paths"]);
    let db = anchored["db_path"].as_str().expect("db_path");
    assert!(
        Path::new(db).starts_with(&project),
        "--root must resolve the store under the named repository, got {db}"
    );

    // Without it, the same invocation answers about the directory it ran in.
    let unanchored = paths_json(&elsewhere, &["--json", "paths"]);
    let other = unanchored["db_path"].as_str().expect("db_path");
    assert_ne!(
        db, other,
        "the anchored and unanchored answers must differ, or this proves nothing"
    );
}

/// `--db` names a store outright, so it still wins where a caller gives both.
#[test]
fn an_explicit_store_still_outranks_the_root() {
    let elsewhere = scratch("both-cwd");
    let project = scratch("both-project");
    let store = scratch("both-store").join("chosen.sqlite");

    let project_arg = project.to_string_lossy().into_owned();
    let store_arg = store.to_string_lossy().into_owned();
    let value = paths_json(
        &elsewhere,
        &[
            "--json",
            "--root",
            &project_arg,
            "--db",
            &store_arg,
            "paths",
        ],
    );
    assert_eq!(
        value["db_path"].as_str().expect("db_path"),
        store_arg,
        "an explicit --db must be used as given"
    );
}

/// The flag is global, so it parses on either side of the subcommand — the same
/// papercut `--db` already documents.
#[test]
fn the_root_flag_parses_on_either_side_of_the_subcommand() {
    let elsewhere = scratch("sides-cwd");
    let project = scratch("sides-project");
    let project_arg = project.to_string_lossy().into_owned();

    let before = paths_json(&elsewhere, &["--json", "--root", &project_arg, "paths"]);
    let after = paths_json(&elsewhere, &["--json", "paths", "--root", &project_arg]);
    assert_eq!(
        before["db_path"], after["db_path"],
        "`devmap --root X paths` and `devmap paths --root X` must agree"
    );
}
