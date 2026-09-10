//! Git HEAD is provenance, not source freshness.
//!
//! `status` used to degrade a generation whose file hashes already matched the
//! working tree whenever `generations.head_sha` lagged `git rev-parse HEAD`.
//! That is the case an empty commit, a skipped rebuild, and a map restamp
//! without a store restamp all produce — and it made `is_fresh: false` a lie
//! that `devmap build` could not clear, because the skip path found nothing to
//! extract and left the stamp behind.
//!
//! Every test here failed against that check. File-hash mismatch must still
//! degrade; HEAD mismatch after a matching hash scan must not.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_resolve::Resolver;
use devmap_store::{current_git_head, GenerationWriteOpts, Store};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-head-stamp-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|error| panic!("cannot spawn git {args:?}: {error}"));
    assert!(status.success(), "git {args:?} failed in {root:?}");
}

fn indexed_git_repo(label: &str) -> (PathBuf, Store, String, u32) {
    let root = tmp_dir(label);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "devmap@example.invalid"]);
    git(&root, &["config", "user.name", "devmap test"]);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "one"]);

    let (files, _report) = extract_tree_with_report(&root).unwrap();
    assert!(
        !files.is_empty(),
        "fixture precondition: discovery indexed the python file"
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files);
    let analysis = analyze(&files, &resolution);
    let store = Store::open_in_memory().unwrap();
    let head = current_git_head(&root).unwrap();
    let generation = store
        .save_generation_with_metadata(
            &files,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
            &head,
        )
        .unwrap();
    let status = store.status("test").unwrap();
    assert!(
        status.is_fresh(),
        "fixture precondition: a just-indexed tree must be fresh, got {:?}",
        status.degraded_reason
    );
    (root, store, head, generation)
}

/// Empty commit: HEAD moved, every indexed byte is the same.
///
/// The old check returned "repository HEAD differs from the indexed
/// generation; rebuild required" here. A rebuild that then compared hashes
/// skipped, so the flag could never clear.
#[test]
fn matching_files_stay_fresh_after_an_empty_commit() {
    let (root, store, first_head, generation) = indexed_git_repo("empty-commit");
    git(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second_head = current_git_head(&root).unwrap();
    assert_ne!(first_head, second_head, "the fixture must move HEAD");
    assert!(
        store.latest_generation_matches_working_tree().unwrap(),
        "hashes must still match after an empty commit"
    );

    let status = store.status("test").unwrap();
    assert_eq!(
        status.latest_generation,
        Some(generation),
        "an empty commit must not invent a generation"
    );
    assert_eq!(
        status.degraded_reason, None,
        "file hashes match, so HEAD lag is provenance, not staleness: {:?}",
        status.degraded_reason
    );
    assert!(
        status.is_fresh(),
        "a matching tree is fresh even when generations.head_sha still names {first_head}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// The hash arm is what freshness is. Changing an indexed file must still
/// degrade, including when HEAD has not moved.
#[test]
fn a_changed_indexed_file_still_degrades_status() {
    let (root, store, _, _) = indexed_git_repo("file-change");
    fs::write(root.join("src/a.py"), "def a():\n    return 99\n").unwrap();
    let status = store.status("test").unwrap();
    assert!(
        !status.is_fresh(),
        "an edited indexed file cannot be fresh: {:?}",
        status.degraded_reason
    );
    assert!(
        status
            .degraded_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("source tree differs")),
        "the reason must name the tree, not HEAD: {:?}",
        status.degraded_reason
    );
    let _ = fs::remove_dir_all(&root);
}

/// Same tree, new commit identity via an orphan branch. The files did not
/// change; only the SHA did.
#[test]
fn an_identical_tree_on_a_new_branch_stays_fresh() {
    let (root, store, first_head, _) = indexed_git_repo("orphan");
    git(&root, &["checkout", "-q", "--orphan", "other"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "other"]);
    let other_head = current_git_head(&root).unwrap();
    assert_ne!(first_head, other_head);

    let status = store.status("test").unwrap();
    assert!(
        status.is_fresh(),
        "an identical tree is fresh under a new SHA: {:?}",
        status.degraded_reason
    );
    let _ = fs::remove_dir_all(&root);
}

/// A stored stamp that is not the current HEAD, including the CLI's
/// `"unavailable"` spelling, must not degrade a matching tree. Git being
/// unreadable is not evidence that the files moved.
#[test]
fn an_unreadable_git_dir_does_not_make_matching_files_stale() {
    let (root, store, _, _) = indexed_git_repo("no-git");
    fs::remove_dir_all(root.join(".git")).unwrap();
    let status = store.status("test").unwrap();
    assert!(
        status.is_fresh(),
        "without git, matching files are still the source of truth: {:?}",
        status.degraded_reason
    );
    let _ = fs::remove_dir_all(&root);
}

/// Many empty commits: the HEAD check used to fail on the first one and stay
/// failed. Status is a scan, so this also stresses that the hash arm stays
/// stable across repeated provenance moves.
#[test]
fn many_empty_commits_leave_a_matching_tree_fresh() {
    let (root, store, first_head, generation) = indexed_git_repo("many-empty");
    for n in 0..32 {
        git(
            &root,
            &["commit", "-q", "--allow-empty", "-m", &format!("empty-{n}")],
        );
        let status = store.status("test").unwrap();
        assert!(
            status.is_fresh(),
            "empty commit {n} made a matching tree look stale: {:?}",
            status.degraded_reason
        );
        assert_eq!(status.latest_generation, Some(generation));
    }
    assert_ne!(current_git_head(&root).unwrap(), first_head);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn restamp_updates_head_without_a_new_generation() {
    let (root, store, first_head, generation) = indexed_git_repo("restamp");
    git(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second_head = current_git_head(&root).unwrap();
    let hashes_before = store.latest_file_hashes().unwrap();
    let nodes_before = store.status("test").unwrap().node_count;

    store.restamp_latest_head(&second_head).unwrap();

    assert_eq!(store.latest_generation_id().unwrap(), Some(generation));
    assert_eq!(
        store.latest_generation_head().unwrap().as_deref(),
        Some(second_head.as_str())
    );
    assert_ne!(second_head, first_head);
    assert_eq!(store.latest_file_hashes().unwrap(), hashes_before);
    assert_eq!(store.status("test").unwrap().node_count, nodes_before);
    assert!(store.status("test").unwrap().is_fresh());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn restamp_is_idempotent_for_the_same_head() {
    let (root, store, head, generation) = indexed_git_repo("restamp-idempotent");
    store.restamp_latest_head(&head).unwrap();
    store.restamp_latest_head(&head).unwrap();
    assert_eq!(store.latest_generation_id().unwrap(), Some(generation));
    assert_eq!(
        store.latest_generation_head().unwrap().as_deref(),
        Some(head.as_str())
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn restamp_rejects_empty_whitespace_and_oversized_stamps() {
    let (root, store, head, _) = indexed_git_repo("restamp-refuse");
    for bad in ["", "abc def", "abc\n", " a", "a ", &"x".repeat(129)] {
        let error = store.restamp_latest_head(bad).unwrap_err().to_string();
        assert!(
            error.contains("head_sha must be non-empty"),
            "rejected {bad:?} with the save_generation sentence, got {error}"
        );
    }
    store.restamp_latest_head(&"y".repeat(128)).unwrap();
    store.restamp_latest_head("unavailable").unwrap();
    store.restamp_latest_head("unknown").unwrap();
    store.restamp_latest_head(&head).unwrap();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn restamp_without_a_generation_refuses() {
    let store = Store::open_in_memory().unwrap();
    let error = store
        .restamp_latest_head("abc1234")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no generation"),
        "an empty store has nothing to restamp: {error}"
    );
}

#[test]
fn concurrent_restamps_serialize_and_leave_one_valid_head() {
    let (root, store, _, generation) = indexed_git_repo("restamp-race");
    let store = std::sync::Arc::new(store);
    let mut joins = Vec::new();
    for i in 0..8 {
        let store = std::sync::Arc::clone(&store);
        joins.push(std::thread::spawn(move || {
            for n in 0..32 {
                let sha = format!("deadbeef{i:02}{n:02}{:028x}", 0u128);
                store.restamp_latest_head(&sha).unwrap();
            }
        }));
    }
    for join in joins {
        join.join().expect("restamp thread");
    }
    assert_eq!(store.latest_generation_id().unwrap(), Some(generation));
    let head = store.latest_generation_head().unwrap().unwrap();
    assert!(
        head.starts_with("deadbeef") && head.len() == 40,
        "the surviving stamp must be one of the restamped values: {head}"
    );
    let _ = fs::remove_dir_all(&root);
}
