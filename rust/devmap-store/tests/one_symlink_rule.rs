//! One symlink rule for the cold walk and for the drain.
//!
//! The two halves of the kernel decided independently what a symlink means and
//! reached opposite answers.
//!
//! - The cold walk ([`devmap_extract::collect_sources_with_report`]) keeps a
//!   symlinked file whose target resolves *inside* the repository — a
//!   monorepo's shared config, a vendored header; the bytes are the
//!   repository's either way — and refuses only one that escapes it, recording
//!   `DiscoverySkipReason::EscapesRoot` as coverage loss.
//! - The drain's `classify_pending_entry` refused **every** symlink, as "not a
//!   regular file or directory", because it asked `symlink_metadata` and read
//!   anything that was neither a plain file nor a plain directory as garbage.
//!
//! Measured against the pre-fix kernel on a repository holding
//! `shared/util.py` and `src/util.py -> shared/util.py`:
//!
//! ```text
//! devmap build .        -> paths: shared/util.py, src/util.py   (both indexed)
//! reconcile_pending_paths(root) with "src/util.py" queued
//!                       -> dropped: [("src/util.py",
//!                                     "not a regular file or directory")]
//! ```
//!
//! The queued edit was deleted as structurally unprocessable, the stored
//! extraction stayed at whatever the cold build had read, and `status` reported
//! fresh — one indexer with two opinions about what the repository contains.
//!
//! The cold walk's rule wins, and it is now stated once, in
//! [`devmap_extract::candidate_kind`]. These tests hold the drain to it.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::Store;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-symlink-rule-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// (a) A queued symlinked source whose target is inside the repository is
/// ordinary work, exactly as the cold walk treats it.
#[test]
fn a_queued_in_root_symlinked_source_is_kept_by_the_drain() {
    let dir = tmp_dir("in-root");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::write(root.join("shared/util.py"), "def util():\n    return 2\n").unwrap();
    std::os::unix::fs::symlink(root.join("shared/util.py"), root.join("src/util.py")).unwrap();

    // The cold walk's answer, read from the owner rather than assumed.
    let (sources, _) = devmap_extract::collect_sources_with_report(&root).unwrap();
    assert!(
        sources.iter().any(|(path, _)| path == "src/util.py"),
        "premise: the cold walk indexes an in-root symlinked source"
    );

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths_under_root(&root, &["src/util.py".to_string()])
        .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();

    assert!(
        outcome.dropped.is_empty(),
        "the drain must not delete work the cold walk would do: {:?}",
        outcome.dropped
    );
    assert_eq!(
        store.get_pending_paths().unwrap(),
        vec!["src/util.py".to_string()],
        "the queued edit has to survive to be drained"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// (b) A queued symlink that escapes the repository is refused for the reason
/// discovery gives, not as an unclassifiable file type. The wording is what an
/// operator reads out of `devmap status`, and "not a regular file or directory"
/// sends them looking for a socket.
#[test]
fn a_queued_escaping_symlink_is_dropped_with_the_discovery_reason() {
    let dir = tmp_dir("escaping");
    let root = dir.join("repo");
    let outside = dir.join("elsewhere");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.py"), "TOKEN = \"hunter2\"\n").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.py"), root.join("src/secret.py")).unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths_under_root(&root, &["src/secret.py".to_string()])
        .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();

    let (path, why) = outcome
        .dropped
        .iter()
        .find(|(path, _)| path == "src/secret.py")
        .unwrap_or_else(|| panic!("the escaping link must be dropped: {:?}", outcome.dropped));
    assert_eq!(path, "src/secret.py");
    assert!(
        why.contains("symlink") && why.contains("secret.py"),
        "the reason must be discovery's — a link the repository does not \
         contain, naming the target — got {why:?}"
    );
    assert!(
        !why.contains("not a regular file or directory"),
        "a symlink is not an unclassifiable file type; got {why:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A dangling link is the same refusal: containment could not be established,
/// and unknown is not "inside".
#[test]
fn a_queued_dangling_symlink_is_refused_the_way_discovery_refuses_it() {
    let dir = tmp_dir("dangling");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    std::os::unix::fs::symlink(root.join("nowhere.py"), root.join("src/gone.py")).unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths_under_root(&root, &["src/gone.py".to_string()])
        .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();

    let (_, why) = outcome
        .dropped
        .iter()
        .find(|(path, _)| path == "src/gone.py")
        .unwrap_or_else(|| panic!("a dangling link must be dropped: {:?}", outcome.dropped));
    assert!(
        why.contains("symlink") && why.contains("could not be resolved"),
        "the reason must say the target would not resolve, got {why:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A queued *directory* symlink is refused too. The walker does not descend a
/// link, so the files under the target are indexed under their real names and
/// only under those; letting the drain expand the link would index the same
/// bytes twice, under two paths the cold walk never both produces.
#[test]
fn a_queued_symlink_to_an_in_root_directory_is_refused_because_the_walk_never_descends_one() {
    let dir = tmp_dir("dirlink");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("shared/pkg")).unwrap();
    fs::write(root.join("shared/pkg/mod.py"), "def m():\n    return 1\n").unwrap();
    std::os::unix::fs::symlink(root.join("shared/pkg"), root.join("pkg")).unwrap();

    let (sources, _) = devmap_extract::collect_sources_with_report(&root).unwrap();
    assert!(
        !sources.iter().any(|(path, _)| path.starts_with("pkg/")),
        "premise: the cold walk does not descend a directory link, got {:?}",
        sources.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths_under_root(&root, &["pkg".to_string()])
        .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();

    let (_, why) = outcome
        .dropped
        .iter()
        .find(|(path, _)| path == "pkg")
        .unwrap_or_else(|| {
            panic!(
                "a directory link the walk never descends is not drain work: {:?}",
                outcome.dropped
            )
        });
    assert!(
        why.contains("symlink") && why.contains("directory"),
        "the reason must say why a directory link is not work, got {why:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The positive control the four above need: an ordinary file is still work,
/// and an ordinary directory is still the whole-subtree rescan the drain
/// expands. A fix that refused everything would pass every assertion above.
#[test]
fn ordinary_files_and_directories_are_untouched_by_the_symlink_rule() {
    let dir = tmp_dir("control");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths_under_root(&root, &["src/a.py".to_string(), "src".to_string()])
        .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();
    assert!(
        outcome.dropped.is_empty(),
        "neither a real source nor a real directory is a symlink: {:?}",
        outcome.dropped
    );
    assert_eq!(
        store.get_pending_paths().unwrap(),
        vec!["src".to_string(), "src/a.py".to_string()],
    );
    let _ = fs::remove_dir_all(&dir);
}
