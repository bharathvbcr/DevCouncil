//! Every `git` the artifact write path runs is bounded in time and in bytes.
//!
//! `freshness::git_head` and `freshness::inventory` (two `ls-files` passes)
//! run on every artifact write. Both called `Command::output()` with no
//! deadline and no cap — the hang `devmap-store` added `GIT_HEAD_DEADLINE` to
//! prevent, reintroduced one crate up. The first two cases here were red
//! against that code: the head digest waited the sleeper's full thirty
//! seconds, and a listing past any size was hashed as the tree.
//!
//! `inventory::churn` already had its bounds; its case is a characterisation
//! that the unified runner keeps them, and it passed before the swap too.
//!
//! Each case stands a shell script in for `git`, so the child's behaviour is
//! the test's to choose. Unix only, because the scripts are.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_query::freshness::{
    self, InventoryLimits, InventorySource, GIT_HEAD_DEADLINE, GIT_LS_FILES_OUTPUT_CAP,
};
use devmap_query::inventory::{self, CHURN_OUTPUT_CAP};

/// One directory per call, unique across the tests this binary runs in
/// parallel: a wall-clock stamp collides at macOS's microsecond resolution.
fn fake_git(body: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("devmap-fakegit-{}-{sequence}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("git");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

#[test]
fn a_stalled_git_cannot_hold_the_head_digest() {
    let (dir, git) = fake_git("exec sleep 30");
    let started = Instant::now();
    let head = freshness::git_head_with_program(git.as_os_str(), &dir);
    let elapsed = started.elapsed();
    assert!(
        head.is_empty(),
        "a git that never answered cannot have produced a head: {head:?}"
    );
    assert!(
        elapsed < GIT_HEAD_DEADLINE + Duration::from_secs(2),
        "the digest must give up at the deadline, not at the sleeper's exit: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A listing the reader could not hold whole is not a listing of the tree, and
/// a fingerprint over a prefix of it would report a stale map as fresh.
#[test]
fn a_listing_past_the_cap_is_refused_rather_than_fingerprinted() {
    let flood = GIT_LS_FILES_OUTPUT_CAP + (1 << 20);
    let (dir, git) = fake_git(&format!("head -c {flood} /dev/zero | tr '\\0' a"));
    let inventory =
        freshness::inventory_with_program(git.as_os_str(), &dir, InventoryLimits::default());
    match &inventory.source {
        InventorySource::Unavailable(reason) => assert!(
            reason.contains("incomplete"),
            "the reason must say the listing was cut, not merely that git failed: {reason}"
        ),
        InventorySource::Git => panic!(
            "a listing past the cap was accepted as the tree: {} files",
            inventory.files.len()
        ),
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A capped `git log` is still a computed answer, says it was cut, and costs
/// the bytes rather than the deadline. Characterisation, not a red test: the
/// reader this replaced closed the pipe at its cap and let `SIGPIPE` end git,
/// which produced the same visible result.
#[test]
fn churn_past_the_cap_costs_its_bytes_not_the_deadline() {
    let flood = CHURN_OUTPUT_CAP + (4 << 20);
    let (dir, git) = fake_git(&format!(
        "yes 'src/busy.py' | tr '\\n' '\\0' | head -c {flood}"
    ));
    let started = Instant::now();
    let churn = inventory::churn_with_program(git.as_os_str(), Path::new("/tmp"));
    let elapsed = started.elapsed();
    assert!(
        churn.computed,
        "the read happened and answered; a capped answer is still an answer: {}",
        churn.unavailable_reason
    );
    assert!(
        churn.truncated,
        "the caller must be told the window was cut at the cap"
    );
    assert!(
        churn
            .commits_by_path
            .get("src/busy.py")
            .copied()
            .unwrap_or(0)
            > 0,
        "the bytes before the cap still count"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "a flood past the cap must not cost the deadline: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
