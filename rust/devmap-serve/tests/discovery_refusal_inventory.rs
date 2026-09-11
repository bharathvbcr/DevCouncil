//! The drain's refusal count is an inventory of paths, not a carried floor.
//!
//! `daemon_discovery_refusals.rs` pins the direction that mattered first: a
//! resync must not erase a refusal the build path recorded. The fix for it
//! carried the previous generation's *count* forward and took
//! `max(previous, this_batch)`, which bought that guarantee with two wrong
//! answers in the other direction:
//!
//! 1. once the refused file is repaired — made readable, a dangling symlink
//!    pointed at a real file — the count stays high until a full re-extraction
//!    or a `devmap build` re-measures. The corpus is complete and the kernel
//!    goes on reporting a hole in it; (Oversized is a default ignore, not a
//!    refusal inventory row — Phase 1.)
//! 2. a refusal this batch met is only *counted* when it exceeds the carried
//!    number, so a new refusal in a repository whose last full walk refused two
//!    files disappears into the two. That one is an over-claim: coverage is
//!    reported as no worse than it already was, when it is worse.
//!
//! Both fall out of the same thing — a count cannot say *which* files. The
//! generation stores one row per refused path, the count is `COUNT(*)` over it,
//! and the drain carries the previous inventory minus every path in this
//! batch's affected set, plus what this batch was turned away from. A path
//! nothing touched keeps its verdict (the watcher would have reported a
//! change); a path this batch touched is re-decided by `candidate_kind`.

use devmap_serve::Daemon;
use devmap_store::Store;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// A scratch repository named for the test using it. No `tempfile`
/// dev-dependency exists in this workspace and this does not add one.
fn scratch(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "devmap-refusal-inventory-{name}-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`) and the daemon
    // canonicalizes its root, so the fixture's must be canonical too.
    dir.canonicalize().unwrap()
}

const LIB: &str = "def helper():\n    return 1\n";
const APP: &str = "from lib import helper\n\n\ndef main():\n    return helper()\n";

/// Exactly what the CLI build path does, refusal inventory included.
fn cold_build(root: &Path, db_path: &Path) -> Store {
    let store = Store::open(db_path).unwrap();
    let (extractions, report) =
        devmap_store::extract_tree_cached_with_report(&store, root).unwrap();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let refusals = devmap_store::discovery_refusals(&report);
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(refusals.len()),
    );
    store
        .save_generation_with_metadata(
            &extractions,
            &resolution,
            &analysis,
            devmap_store::GenerationWriteOpts {
                discovery_refusals: Some(refusals),
                ..Default::default()
            },
            // The stamp the daemon's own HEAD reading produces outside a git
            // repository. Without it `head_moved` is true on the first drain
            // and every test here exercises the full-rebuild branch, which
            // re-walks discovery and so measures nothing this file is about.
            "unavailable",
        )
        .unwrap();
    store
}

fn refused_paths(store: &Store) -> Vec<String> {
    store
        .latest_discovery_refusals()
        .unwrap()
        .into_iter()
        .map(|refusal| refusal.path)
        .collect()
}

fn refused_count(store: &Store) -> Option<usize> {
    store
        .latest_analysis()
        .unwrap()
        .expect("a generation is persisted")
        .discovery_refused_files
}

fn enqueue_and_drain(store: &Store, daemon: &Daemon, root: &Path, paths: &[&str]) {
    let absolute: Vec<String> = paths
        .iter()
        .map(|path| root.join(path).to_string_lossy().into_owned())
        .collect();
    store.enqueue_pending_paths(&absolute).unwrap();
    daemon
        .drain_pending_batch()
        .expect("the resync must succeed");
}

/// Residual 1: a repaired file must leave the inventory.
#[test]
#[cfg(unix)]
fn a_repaired_unreadable_file_leaves_the_refusal_inventory() {
    let root = scratch("unreadable");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    let app = root.join("app.py");
    std::fs::write(&app, APP).unwrap();
    let mut perms = std::fs::metadata(&app).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&app, perms).unwrap();
    let db_path = root.join("index.sqlite");

    let store = cold_build(&root, &db_path);
    assert_eq!(
        refused_count(&store),
        Some(1),
        "fixture precondition: the cold walk must refuse the unreadable file"
    );
    assert_eq!(
        refused_paths(&store),
        vec!["app.py".to_string()],
        "the inventory must name the path, not just count it"
    );

    // Repaired: the file is readable again and is an ordinary source.
    let mut perms = std::fs::metadata(&app).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&app, perms).unwrap();
    std::fs::write(&app, APP).unwrap();
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    enqueue_and_drain(&store, &daemon, &root, &["app.py"]);

    assert_eq!(
        refused_paths(&store),
        Vec::<String>::new(),
        "the drain re-decided this very path and read it in full, so its refusal \
         row must be gone"
    );
    assert_eq!(
        refused_count(&store),
        Some(0),
        "a corpus with nothing left unread must say so. Carrying the previous \
         count as a floor leaves a repaired repository permanently reported as \
         partly unread, and the dead-code pass permanently capped below the \
         tier `CLAUDE.md` tells agents to act on"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Residual 2, and the carry rule itself.
///
/// The first drain meets a path the last full walk never refused, so the count
/// must rise. The second drain touches an unrelated file and must leave that
/// verdict exactly where it was — asserted on the inventory rather than on the
/// count, because a floor keeps the count right for the wrong reason.
#[test]
fn a_new_refusal_is_recorded_by_path_and_an_unrelated_batch_leaves_it_alone() {
    let root = scratch("new-refusal");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    std::fs::write(root.join("app.py"), APP).unwrap();
    let db_path = root.join("index.sqlite");

    let store = cold_build(&root, &db_path);
    assert_eq!(
        refused_count(&store),
        Some(0),
        "fixture precondition: this walk refused nothing"
    );

    // A symlink out of the repository. `candidate_kind` refuses it, and a
    // `devmap build` of the same tree would write no rows for it.
    let outside = scratch("new-refusal-outside").join("secret.py");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, "def secret():\n    return 0\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("escape.py")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, root.join("escape.py")).unwrap();

    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    enqueue_and_drain(&store, &daemon, &root, &["escape.py"]);

    assert_eq!(
        refused_paths(&store),
        vec!["escape.py".to_string()],
        "a refusal this batch met must be recorded under its own name"
    );

    // An unrelated edit. Nothing changed about `escape.py`, so its verdict
    // stands — but it must stand because the row was carried, not because a
    // floor happened to hold the number up.
    std::fs::write(root.join("lib.py"), "def helper():\n    return 11\n").unwrap();
    enqueue_and_drain(&store, &daemon, &root, &["lib.py"]);

    assert_eq!(
        refused_paths(&store),
        vec!["escape.py".to_string()],
        "the batch did not touch `escape.py`, so the previous generation's \
         verdict on it is still the best answer the kernel has"
    );
    assert_eq!(refused_count(&store), Some(1));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(outside.parent().unwrap());
}

/// End to end, in the shape of `one_symlink_rule_end_to_end.rs`: a dangling
/// symlink refused at cold build, repaired into a real file, drained.
///
/// The refusal must clear *and* the file's symbols must be in the graph. Only
/// asserting the count would pass on a kernel that cleared the number without
/// reading the file, which is the opposite failure and the worse one.
#[test]
fn a_repaired_symlink_clears_the_refusal_and_lands_in_the_graph() {
    let root = scratch("repaired-symlink");
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("nowhere.py"), root.join("app.py")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(root.join("nowhere.py"), root.join("app.py")).unwrap();
    let db_path = root.join("index.sqlite");

    let store = cold_build(&root, &db_path);
    assert_eq!(
        refused_paths(&store),
        vec!["app.py".to_string()],
        "fixture precondition: a dangling link is a refusal, not an absence"
    );

    // The link now points at a real file inside the repository.
    std::fs::write(root.join("nowhere.py"), APP).unwrap();
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    enqueue_and_drain(&store, &daemon, &root, &["app.py", "nowhere.py"]);

    assert_eq!(
        refused_paths(&store),
        Vec::<String>::new(),
        "the link resolves inside the repository now: {:?}",
        store.latest_discovery_refusals().unwrap()
    );
    assert_eq!(refused_count(&store), Some(0));

    let symbols: Vec<String> = store
        .all_symbols()
        .unwrap()
        .into_iter()
        .map(|symbol| format!("{}::{}", symbol.path, symbol.name))
        .collect();
    assert!(
        symbols.iter().any(|entry| entry.ends_with("::main")),
        "the repaired file was read, so `main` belongs to the graph: {symbols:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
