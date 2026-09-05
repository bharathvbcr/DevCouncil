//! Races and refusals around one drain generation.
//!
//! Everything here drives `Daemon::drain_pending_batch` against a real
//! repository on disk and a real SQLite store, opened a second time so the
//! test can enqueue and read back without widening the daemon's private
//! `store` field — the same trade `daemon_discovery_refusals.rs` records.
//!
//! No `tempfile` dev-dependency exists in this workspace and nothing here adds
//! one; scratch trees are named after the test plus a nanosecond stamp.

use devmap_serve::Daemon;
use devmap_store::Store;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "devmap-drain-race-{name}-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`) and the daemon
    // canonicalizes its root, so the fixture's must be canonical too.
    dir.canonicalize().unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|error| panic!("cannot spawn git {args:?}: {error}"));
    assert!(status.success(), "git {args:?} failed in {root:?}");
}

/// A repository with one commit, and a store holding one generation stamped at
/// that commit. Returns the daemon and a second handle on the same store.
fn repo_with_one_generation(name: &str) -> (PathBuf, Daemon, Store) {
    let root = scratch(name);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "devmap@example.invalid"]);
    git(&root, &["config", "user.name", "devmap test"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "one"]);

    let db_path = root.join("index.sqlite");
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    let reader = Store::open(&db_path).unwrap();

    assert_eq!(
        daemon.reconcile_connect_time().unwrap(),
        1,
        "the sweep must enqueue the one source file"
    );
    assert_eq!(
        daemon.drain_pending_batch().unwrap(),
        1,
        "the first drain must write a generation"
    );
    (root, daemon, reader)
}

fn head_of(root: &Path) -> String {
    devmap_store::current_git_head(root).expect("the fixture is a git repository")
}

fn generation_paths(store: &Store) -> Vec<String> {
    let mut paths: Vec<String> = store
        .latest_extractions()
        .unwrap()
        .into_iter()
        .map(|extraction| extraction.file_path)
        .collect();
    paths.sort();
    paths
}

/// A generation written after HEAD moved must describe the tree at that HEAD,
/// not a carry-forward of the previous checkout.
///
/// The carry-forward branch reuses `latest_extractions()` for every file this
/// batch did not name. That is only sound while the checkout has not changed
/// underneath it. The move check was gated on a git-HEAD *sentinel* being in
/// the same claimed batch — `head_event && head_differs_from_last_generation`
/// — so a drain that ran on an ordinary file event while HEAD had already
/// moved carried the old checkout's rows forward **and then stamped the new
/// generation with the current HEAD**. That stamp is what makes the damage
/// permanent: the sentinel arrives in the next batch, compares the current
/// HEAD against the one just stored, finds them equal, and never rebuilds.
///
/// `src/b.py` exists on disk, is not in the previous generation, and is not in
/// the claimed batch. Only a full re-extraction can put it in the generation,
/// so it is the observable difference between "carried the old checkout
/// forward" and "re-read the tree at this HEAD".
#[test]
fn a_generation_written_after_head_moved_re_reads_the_tree() {
    let (root, daemon, reader) = repo_with_one_generation("head-moved");
    let first_head = head_of(&root);
    assert_eq!(
        reader.latest_generation_head_sha().unwrap().as_deref(),
        Some(first_head.as_str()),
        "the first generation must be stamped at the commit it was built from"
    );

    // A file the drain is never told about. A checkout that adds files the
    // watcher's events for were lost or not yet drained looks exactly like
    // this from the store's side.
    std::fs::write(root.join("src/b.py"), "def b():\n    return 2\n").unwrap();

    // HEAD moves. No sentinel is enqueued: the watcher is not running, which
    // is the ordinary case for a drain triggered by the connect-time sweep or
    // by an event batch the sentinel did not make it into.
    git(&root, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second_head = head_of(&root);
    assert_ne!(first_head, second_head, "the fixture must move HEAD");

    std::fs::write(root.join("src/a.py"), "def a():\n    return 99\n").unwrap();
    reader
        .enqueue_pending_paths_under_root(&root, &["src/a.py".to_string()])
        .unwrap();

    assert_eq!(daemon.drain_pending_batch().unwrap(), 1);

    assert_eq!(
        generation_paths(&reader),
        vec!["src/a.py".to_string(), "src/b.py".to_string()],
        "HEAD moved since the stored generation, so the rows carried forward from \
         it describe a checkout that no longer exists; the drain must re-read the \
         tree instead of stamping the new HEAD onto the old rows"
    );
    assert_eq!(
        reader.latest_generation_head_sha().unwrap().as_deref(),
        Some(second_head.as_str()),
        "and the generation must be stamped at the HEAD it was built against"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A store that has been corrupted underneath the daemon must refuse, never
/// answer "empty and up to date".
///
/// `index_is_fresh` is `latest_generation.is_some() && pending_count == 0`, and
/// both halves are read out of the store. A store whose pages have been
/// overwritten can fail those reads — and the one answer it must never produce
/// is the shape of a healthy, quiet, fully-indexed repository, because that is
/// what `devmap_client.is_map_stale()` reads as "no work to do". Failure must
/// not look like emptiness.
///
/// The corruption is applied to the file while a handle is open, which is the
/// real shape of it: a truncated write, a full disk, a container image swapped
/// under a running process. Measured behaviour on this workspace: `Store::open`
/// refuses with `file is not a database`, so the refusal happens at the first
/// branch below. The other two branches are kept because which one answers is
/// SQLite's choice, not this kernel's — a page still in cache, a `-wal` that
/// survived — and every one of them has to refuse.
#[test]
fn a_corrupted_store_refuses_rather_than_answering_empty_and_fresh() {
    let (root, daemon, reader) = repo_with_one_generation("corrupt-store");
    let db_path = root.join("index.sqlite");

    // Sanity: healthy first, or the assertion below could pass for the wrong
    // reason.
    let healthy = reader.status("test").expect("a healthy store answers");
    assert!(
        devmap_serve::index_is_fresh(&healthy),
        "the fixture must start fresh, got {healthy:?}"
    );

    // Overwrite the whole file with bytes that are not a SQLite database,
    // keeping the length so the page cache cannot hide the damage behind a
    // short-file check.
    let length = std::fs::metadata(&db_path).unwrap().len() as usize;
    std::fs::write(&db_path, vec![0x5a; length]).unwrap();
    for suffix in ["-wal", "-shm"] {
        let side = db_path.with_extension(format!("sqlite{suffix}"));
        if side.exists() {
            let side_length = std::fs::metadata(&side).unwrap().len() as usize;
            std::fs::write(&side, vec![0x5a; side_length]).unwrap();
        }
    }

    // A fresh handle is what a newly spawned client gets, and it cannot be
    // served by anybody's page cache.
    match Store::open(&db_path) {
        Err(_) => {} // Refused at open: loud, and correct.
        Ok(store) => match store.status("test") {
            Err(_) => {} // Refused at read: loud, and correct.
            Ok(status) => {
                assert!(
                    !devmap_serve::index_is_fresh(&status),
                    "a store that cannot be read must not answer with the exact shape \
                     of a healthy, fully-indexed one — that is what the client reads \
                     as `nothing to do`. Got {status:?}"
                );
                assert!(
                    devmap_serve::freshness_degraded_reason(&status).is_some(),
                    "and a store reported not-fresh must say why, or the caller is \
                     told the index is stale with no way to find out what happened"
                );
            }
        },
    }

    // The drain must not report progress against a store it cannot write.
    if let Ok(drained) = daemon.drain_pending_batch() {
        assert_eq!(
            drained, 0,
            "a drain that could not persist anything must not report paths drained"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The OFF direction. A drain at an unmoved HEAD must still take the cheap
/// carry-forward path: a fix that answered "re-read everything" to every drain
/// would satisfy the test above and undo the whole point of the incremental
/// path.
#[test]
fn a_generation_written_at_an_unmoved_head_still_carries_forward() {
    let (root, daemon, reader) = repo_with_one_generation("head-still");
    let head = head_of(&root);

    // Same shape as the test above, minus the commit.
    std::fs::write(root.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    std::fs::write(root.join("src/a.py"), "def a():\n    return 99\n").unwrap();
    reader
        .enqueue_pending_paths_under_root(&root, &["src/a.py".to_string()])
        .unwrap();

    assert_eq!(daemon.drain_pending_batch().unwrap(), 1);

    assert_eq!(
        generation_paths(&reader),
        vec!["src/a.py".to_string()],
        "HEAD did not move, so the batch must cost exactly the one file it named — \
         a drain that re-walks the tree every time is not an incremental drain"
    );
    assert_eq!(
        reader.latest_generation_head_sha().unwrap().as_deref(),
        Some(head.as_str())
    );
    let _ = std::fs::remove_dir_all(&root);
}
