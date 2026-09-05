//! `build --manifest`: one process, and no rewrite of artifacts that would not
//! change.
//!
//! The seam ran `build`, then `manifest`, then `status` on every PostToolUse
//! hook — three processes and three store opens for one question about one
//! generation — and `manifest` re-read the whole generation out of SQLite and
//! re-serialized a 22 MB `code_graph.json` each time, to produce bytes identical
//! to the ones already on disk. `write_atomic` then compared them and declined
//! the rename, so the work was not merely wasted, it was *provably* wasted:
//! 0.42 s of a 1.39 s hook on an unchanged tree.
//!
//! What these tests hold:
//!  - the fused command writes both artifacts and reports the store's status,
//!    so the seam needs no second and third process;
//!  - a second run over an unchanged tree reports `artifacts_unchanged` and
//!    leaves both files byte-, mtime- and inode-identical;
//!  - every input that can change the artifacts' content invalidates the skip;
//!  - the artifacts a skipped run leaves behind are the ones a fresh write
//!    would have produced.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

fn run_json(root: &Path, args: &[&str]) -> serde_json::Value {
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

/// A small git repository, because the freshness digests are computed from
/// `git ls-files` and a directory that is not a work tree cannot answer.
fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-manifest-once-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/a.py"),
        "def helper():\n    return 1\n\n\ndef caller():\n    return helper()\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/b.py"),
        "from a import helper\n\n\ndef second():\n    return helper()\n",
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

/// `(len, mtime_ns, inode)` — the identity a skip must leave untouched.
fn identity(path: &Path) -> (u64, i128, u64) {
    let meta = std::fs::metadata(path).unwrap_or_else(|error| {
        panic!("{} must exist: {error}", path.display());
    });
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128;
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0u64;
    (meta.len(), mtime, ino)
}

fn db(root: &Path) -> String {
    root.join(".devcouncil/codeintel/devmap.sqlite")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn one_invocation_writes_the_artifacts_and_reports_the_store() {
    let root = fixture("fused");
    let db = db(&root);
    let built = run_json(&root, &["--db", &db, "build", ".", "--manifest", "--force"]);

    let manifest = &built["manifest"];
    assert!(
        manifest.is_object(),
        "build --manifest must report what it wrote: {built}"
    );
    assert_eq!(manifest["artifacts_unchanged"], serde_json::json!(false));
    // The digests came from the kernel, not from flags nobody passed.
    assert_eq!(manifest["freshness_source"], serde_json::json!("kernel"));
    // …and the store's own view travels with them, so the seam does not need a
    // third process to ask.
    assert!(
        manifest["status"]["is_fresh"].is_boolean(),
        "the fused result must carry the store status: {manifest}"
    );
    assert_eq!(manifest["status"]["generation_id"], built["generation_id"]);

    let map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".devcouncil/repo_map.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(map["map_engine"], serde_json::json!("devmap-rust"));
    // The kernel stamped the freshness fields itself: they are what
    // `RepoMapper.map_is_stale` compares, and an unstamped map reads stale
    // forever.
    assert_eq!(
        map["content_fingerprint"]
            .as_str()
            .map(|value| value.starts_with("c2:")),
        Some(true),
        "content_fingerprint must be stamped: {}",
        map["content_fingerprint"]
    );
    assert_eq!(map["indexed_hash"].as_str().map(str::len), Some(40));
    assert_eq!(map["generated_head"].as_str().map(str::len), Some(40));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_second_run_over_an_unchanged_tree_rewrites_nothing() {
    let root = fixture("unchanged");
    let db = db(&root);
    run_json(&root, &["--db", &db, "build", ".", "--manifest", "--force"]);

    let map_path = root.join(".devcouncil/repo_map.json");
    let graph_path = root.join(".devcouncil/graph/code_graph.json");
    let before = (identity(&map_path), identity(&graph_path));

    let again = run_json(&root, &["--db", &db, "build", ".", "--manifest", "--force"]);
    assert_eq!(
        again["unchanged"],
        serde_json::json!(true),
        "the build itself must take the unchanged path: {again}"
    );
    assert_eq!(
        again["manifest"]["artifacts_unchanged"],
        serde_json::json!(true),
        "the artifacts must be recognised as already current: {}",
        again["manifest"]
    );
    assert_eq!(
        before,
        (identity(&map_path), identity(&graph_path)),
        "a no-op manifest must not touch either artifact's length, mtime or inode"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The skip must fire only when *every* input still holds. Each case here moves
/// exactly one of them.
#[test]
fn every_input_that_changes_the_artifacts_defeats_the_skip() {
    let root = fixture("invalidate");
    let db = db(&root);
    let base: Vec<&str> = vec!["--db", &db, "build", ".", "--manifest", "--force"];
    run_json(&root, &base);
    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        serde_json::json!(true),
        "baseline: the second run must skip"
    );

    // 1. An artifact edited under us is not the artifact we wrote.
    std::fs::write(
        root.join(".devcouncil/repo_map.json"),
        "{\"map_engine\":\"devmap-rust\"}",
    )
    .unwrap();
    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        serde_json::json!(false),
        "a tampered artifact must be rewritten"
    );

    // 2. An artifact deleted must be rewritten.
    std::fs::remove_file(root.join(".devcouncil/graph/code_graph.json")).unwrap();
    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        serde_json::json!(false),
        "a missing artifact must be rewritten"
    );
    assert!(root.join(".devcouncil/graph/code_graph.json").is_file());

    // 3. A caller-supplied stamp is a different identity from the kernel's own.
    let stamped = run_json(
        &root,
        &[
            "--db",
            &db,
            "build",
            ".",
            "--manifest",
            "--force",
            "--generated-head",
            "deadbeef",
        ],
    );
    assert_eq!(
        stamped["manifest"]["artifacts_unchanged"],
        serde_json::json!(false),
        "different stamps must rewrite"
    );
    assert_eq!(
        stamped["manifest"]["freshness_source"],
        serde_json::json!("caller")
    );
    let map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".devcouncil/repo_map.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(map["generated_head"], serde_json::json!("deadbeef"));
    // Supplying one digest switches the whole set: mixing a caller's head with
    // the kernel's own two would stamp an identity no snapshot of the tree had.
    assert_eq!(map["indexed_hash"], serde_json::json!(""));

    // 4. A changed tree is a new generation, so a new artifact.
    std::fs::write(root.join("src/c.py"), "def third():\n    return 3\n").unwrap();
    let changed = run_json(&root, &base);
    assert_eq!(
        changed["manifest"]["artifacts_unchanged"],
        serde_json::json!(false),
        "a new generation must rewrite: {changed}"
    );

    // 5. …and settles again.
    assert_eq!(
        run_json(&root, &base)["manifest"]["artifacts_unchanged"],
        serde_json::json!(true)
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A skipped run must leave the artifacts a written run would have produced —
/// otherwise the skip is not an optimization, it is a stale answer.
#[test]
fn the_skipped_artifacts_equal_the_ones_a_forced_write_produces() {
    let root = fixture("equivalence");
    let db = db(&root);
    let base: Vec<&str> = vec!["--db", &db, "build", ".", "--manifest", "--force"];
    run_json(&root, &base);
    let map_after_write = std::fs::read(root.join(".devcouncil/repo_map.json")).unwrap();
    let graph_after_write = std::fs::read(root.join(".devcouncil/graph/code_graph.json")).unwrap();

    let skipped = run_json(&root, &base);
    assert_eq!(
        skipped["manifest"]["artifacts_unchanged"],
        serde_json::json!(true)
    );

    // Remove the stamp so the next run cannot skip, and compare what it writes
    // against what the skip left in place.
    std::fs::remove_file(format!("{db}.artifacts.json")).unwrap();
    let rewritten = run_json(&root, &["--db", &db, "manifest", ".", "--force"]);
    assert_eq!(
        rewritten["artifacts_unchanged"],
        serde_json::json!(false),
        "without a stamp there is nothing to skip on"
    );
    assert_eq!(
        map_after_write,
        std::fs::read(root.join(".devcouncil/repo_map.json")).unwrap(),
        "the map a skip preserved differs from the one a write produces"
    );
    assert_eq!(
        graph_after_write,
        std::fs::read(root.join(".devcouncil/graph/code_graph.json")).unwrap(),
        "the graph a skip preserved differs from the one a write produces"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `devmap freshness` answers without a store, names which field moved, and
/// fails closed when it cannot enumerate.
#[test]
fn freshness_answers_field_by_field_and_fails_closed() {
    let root = fixture("freshness");

    // No store anywhere: the question is about the working tree.
    let now = run_json(&root, &["freshness", "."]);
    assert_eq!(now["source"], serde_json::json!("git"));
    assert_eq!(now["stale"], serde_json::Value::Null, "nothing was asked");
    let head = now["generated_head"].as_str().unwrap().to_string();
    let indexed = now["indexed_hash"].as_str().unwrap().to_string();
    let content = now["content_fingerprint"].as_str().unwrap().to_string();
    assert_eq!(now["files"], serde_json::json!(2));

    let matching = run_json(
        &root,
        &[
            "freshness",
            ".",
            "--expect-head",
            &head,
            "--expect-indexed-hash",
            &indexed,
            "--expect-content-fingerprint",
            &content,
        ],
    );
    assert_eq!(matching["stale"], serde_json::json!(false));
    assert_eq!(
        matching["checked"]["content"]["match"],
        serde_json::json!(true)
    );

    // Edit a file: content moves, the inventory does not.
    std::fs::write(root.join("src/a.py"), "def helper():\n    return 2\n").unwrap();
    let edited = run_json(
        &root,
        &[
            "freshness",
            ".",
            "--expect-head",
            &head,
            "--expect-indexed-hash",
            &indexed,
            "--expect-content-fingerprint",
            &content,
        ],
    );
    assert_eq!(edited["stale"], serde_json::json!(true));
    assert_eq!(
        edited["checked"]["inventory"]["match"],
        serde_json::json!(true)
    );
    assert_eq!(
        edited["checked"]["content"]["match"],
        serde_json::json!(false)
    );
    assert!(
        edited["reason"].as_str().unwrap().contains("content"),
        "the reason must name the field that moved: {}",
        edited["reason"]
    );

    // Add a file: the inventory moves too.
    std::fs::write(root.join("src/d.py"), "D = 1\n").unwrap();
    let added = run_json(
        &root,
        &["freshness", ".", "--expect-indexed-hash", &indexed],
    );
    assert_eq!(added["stale"], serde_json::json!(true));
    assert_eq!(
        added["checked"]["inventory"]["match"],
        serde_json::json!(false)
    );

    // A directory that is not a work tree cannot prove anything fresh, so it
    // must answer stale, with the reason — never fresh by absence of evidence.
    let outside = std::env::temp_dir().join(format!(
        "devmap-not-a-repo-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&outside).unwrap();
    let unavailable = run_json(
        &outside,
        &["freshness", ".", "--expect-indexed-hash", &indexed],
    );
    if unavailable["source"] == serde_json::json!("unavailable") {
        assert_eq!(unavailable["stale"], serde_json::json!(true));
        assert!(!unavailable["reason"].as_str().unwrap().is_empty());
        assert_eq!(unavailable["indexed_hash"], serde_json::Value::Null);
    } else {
        // The machine's temp directory is itself inside a repository; the case
        // this asserts could not be reached, and saying so beats a green tick.
        eprintln!(
            "NOT CHECKED: {} is inside a git work tree, so the unavailable path was not exercised",
            outside.display()
        );
    }

    let _ = std::fs::remove_dir_all(&outside);
    let _ = std::fs::remove_dir_all(&root);
}

/// A field whose check was skipped must not report what a field that was
/// checked and passed reports.
///
/// `map_is_stale` computes the content fingerprint only once head and inventory
/// both match, because hashing the whole inventory costs several times what the
/// two `git ls-files` passes that already answered cost. This command has to
/// short-circuit the same way — and then say so, rather than emit a `match` for
/// a comparison it never made.
#[test]
fn a_skipped_content_check_says_it_was_skipped() {
    let root = fixture("shortcircuit");
    let now = run_json(&root, &["freshness", "."]);
    let indexed = now["indexed_hash"].as_str().unwrap().to_string();
    let content = now["content_fingerprint"].as_str().unwrap().to_string();

    // Head and inventory still match: the content check runs.
    let checked = run_json(
        &root,
        &[
            "freshness",
            ".",
            "--expect-indexed-hash",
            &indexed,
            "--expect-content-fingerprint",
            &content,
        ],
    );
    assert_eq!(
        checked["checked"]["content"]["match"],
        serde_json::json!(true)
    );
    assert_eq!(checked["content_fingerprint"], serde_json::json!(content));

    // The inventory moved, so the content check is not run at all.
    std::fs::write(root.join("src/new.py"), "N = 1\n").unwrap();
    let skipped = run_json(
        &root,
        &[
            "freshness",
            ".",
            "--expect-indexed-hash",
            &indexed,
            "--expect-content-fingerprint",
            &content,
        ],
    );
    assert_eq!(skipped["stale"], serde_json::json!(true));
    assert_eq!(
        skipped["checked"]["content"]["checked"],
        serde_json::json!(false)
    );
    assert!(
        skipped["checked"]["content"].get("match").is_none(),
        "a skipped check must carry no verdict: {}",
        skipped["checked"]["content"]
    );
    assert_eq!(
        skipped["content_fingerprint"],
        serde_json::Value::Null,
        "a digest that was not computed is null, not an empty string"
    );
    // …and the reason names the field that actually moved, not the one that
    // was never looked at.
    assert!(
        skipped["reason"].as_str().unwrap().contains("inventory")
            && !skipped["reason"].as_str().unwrap().contains("content"),
        "{}",
        skipped["reason"]
    );

    let _ = std::fs::remove_dir_all(&root);
}
