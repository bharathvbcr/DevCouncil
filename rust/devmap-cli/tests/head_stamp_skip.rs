use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn scratch(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "devmap-head-skip-{label}-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    root.canonicalize().unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .expect("devmap invocation")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn git_repo_with_source(label: &str) -> PathBuf {
    let root = scratch(label);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "devmap@example.invalid"]);
    git(&root, &["config", "user.name", "devmap test"]);
    std::fs::write(root.join("src/a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(root.join("src/b.py"), "def helper():\n    return 1\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "one"]);
    root
}

fn status_json(root: &Path) -> serde_json::Value {
    let output = run(root, &["--json", "status"]);
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(stdout(&output).trim()).expect("status json")
}

fn stored_head(root: &Path) -> String {
    let store = devmap_store::Store::open(devmap_extract::paths::store_path(root)).unwrap();
    store
        .latest_generation_head()
        .unwrap()
        .expect("a built store has a head stamp")
}

fn git_head(root: &Path) -> String {
    devmap_store::current_git_head(root).expect("fixture is a git repository")
}

/// An empty commit used to make `devmap build` skip *and* leave `status`
/// NOT FRESH forever: hashes matched, so nothing was written, so the HEAD
/// stamp stayed on the previous commit, so the status HEAD check kept
/// demanding a rebuild that would skip again.
#[test]
fn an_empty_commit_skip_restamps_head_and_reports_fresh() {
    let root = git_repo_with_source("empty-skip");
    let first = run(&root, &["build", "."]);
    assert!(
        first.status.success(),
        "cold build failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_head = git_head(&root);
    assert_eq!(stored_head(&root), first_head);
    let generation = status_json(&root)["generation_id"].clone();
    assert_eq!(status_json(&root)["is_fresh"], true);

    git(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second_head = git_head(&root);
    assert_ne!(first_head, second_head);

    // Status itself must not call matching files stale. The rebuild is not
    // required; if this fails, the HEAD-after-hash-match check is still live.
    let before_skip = status_json(&root);
    assert_eq!(
        before_skip["is_fresh"], true,
        "matching files are fresh before the skip restamps provenance: {before_skip}"
    );

    let skipped = run(&root, &["build", "."]);
    assert!(
        skipped.status.success(),
        "skip build failed: {}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    let skip_out = stdout(&skipped);
    assert!(
        skip_out.contains("No source changes"),
        "an empty commit must skip: {skip_out}"
    );

    let after = status_json(&root);
    assert_eq!(after["is_fresh"], true, "{after}");
    assert_eq!(after["generation_id"], generation, "{after}");
    assert_eq!(after["degraded_reason"], serde_json::Value::Null, "{after}");
    assert_eq!(
        stored_head(&root),
        second_head,
        "the skip must restamp generations.head_sha to the current HEAD"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `--manifest` used to restamp the map's `generated_head` from git and leave
/// `freshness.head_sha` (the store) on the previous commit. Agents reading
/// either half then disagreed with each other.
#[test]
fn a_manifest_skip_keeps_map_and_store_heads_aligned() {
    let root = git_repo_with_source("manifest-skip");
    let first = run(&root, &["build", ".", "--manifest"]);
    assert!(
        first.status.success(),
        "cold --manifest failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    git(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second_head = git_head(&root);
    let skipped = run(&root, &["build", ".", "--manifest"]);
    assert!(
        skipped.status.success(),
        "skip --manifest failed: {}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    assert!(
        stdout(&skipped).contains("No source changes"),
        "{}",
        stdout(&skipped)
    );

    let map_path = devmap_extract::paths::repo_map_path(&root);
    let map: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&map_path).unwrap()).unwrap();
    let map_head = map["freshness"]["head_sha"].as_str().unwrap_or("");
    let generated = map["generated_head"].as_str().unwrap_or("");
    assert_eq!(
        stored_head(&root),
        second_head,
        "store head must follow the skip"
    );
    assert_eq!(
        map_head, second_head,
        "map freshness.head_sha must match the restamped store, not the previous commit"
    );
    assert_eq!(
        generated, second_head,
        "generated_head must not drift from the store"
    );
    assert_eq!(status_json(&root)["is_fresh"], true);
    let _ = std::fs::remove_dir_all(&root);
}

/// A real source edit after an empty commit must still rebuild. The skip is
/// only for an unchanged tree; HEAD moving is not a substitute for hashing.
#[test]
fn a_source_edit_after_an_empty_commit_still_rebuilds() {
    let root = git_repo_with_source("edit-after-empty");
    assert!(run(&root, &["build", "."]).status.success());
    git(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
    std::fs::write(
        root.join("src/b.py"),
        "def helper():\n    return 1\n\ndef added():\n    return 2\n",
    )
    .unwrap();
    let output = stdout(&run(&root, &["build", "."]));
    assert!(
        !output.contains("No source changes"),
        "an edited file must rebuild even when HEAD also moved: {output}"
    );
    assert_eq!(status_json(&root)["is_fresh"], true);
    let _ = std::fs::remove_dir_all(&root);
}
