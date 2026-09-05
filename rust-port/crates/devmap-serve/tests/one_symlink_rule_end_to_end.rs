//! The cold walk and the drain, on the same repository, must agree about what
//! a symlink is.
//!
//! `devmap_extract::collect_sources_with_report` — the walk behind
//! `devmap build` — keeps a symlinked file whose target resolves inside the
//! repository and refuses one that escapes it. The daemon's drain answered both
//! differently: `classify_pending_entry` deleted every queued symlink as "not a
//! regular file or directory", and a row that reached
//! `collect_pending_path_with` anyway was met with
//! `bail!("watched file resolves outside daemon root")` — a failure, which
//! charges the path a retry and quarantines it after five, while the rows the
//! build path had already stopped writing stayed in the generation.
//!
//! Measured against the pre-fix kernel:
//!
//! ```text
//! # src/util.py -> shared/util.py, both inside the repository
//! devmap build .                       -> src/util.py indexed (util_v1)
//! <edit shared/util.py to define util_v2>
//! reconcile_pending_paths(root)        -> dropped: [("src/util.py",
//!                                          "not a regular file or directory")]
//! drain                                -> src/util.py still says util_v1
//!
//! # src/creds.py replaced by a link out of the repository
//! drain                                -> Err: all 1 claimed path(s) failed:
//!                                          watched file resolves outside daemon root
//! ```
//!
//! Both halves now ask `devmap_extract::candidate_kind`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::model::AnalysisStatus;
use devmap_serve::Daemon;
use devmap_store::Store;

fn scratch(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "devmap-symlink-e2e-{name}-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`) and the daemon
    // canonicalizes its root, so the fixture's must be canonical too.
    dir.canonicalize().unwrap()
}

/// Exactly what the CLI build path does, refusal count included.
fn cold_build(root: &Path, db_path: &Path) -> Store {
    let store = Store::open(db_path).unwrap();
    let (extractions, report) =
        devmap_store::extract_tree_cached_with_report(&store, root).unwrap();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(report.refused_count()),
    );
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    store
}

fn symbols_of(store: &Store, path: &str) -> Vec<String> {
    store
        .latest_extractions()
        .unwrap()
        .into_iter()
        .filter(|extraction| extraction.file_path == path)
        .flat_map(|extraction| {
            extraction
                .symbols
                .into_iter()
                .map(|symbol| symbol.name)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// (c), the keeping half: a cold build indexes an in-repository symlink, and a
/// drain of that same path must carry the edit through instead of deleting the
/// queued row.
#[test]
fn a_cold_build_indexes_an_in_root_symlink_and_a_drain_of_it_keeps_the_symbol() {
    let root = scratch("in-root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("shared")).unwrap();
    std::fs::write(
        root.join("shared/util.py"),
        "def util_v1():\n    return 1\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("shared/util.py"), root.join("src/util.py")).unwrap();
    let db_path = root.join("index.sqlite");

    let store = cold_build(&root, &db_path);
    assert!(
        symbols_of(&store, "src/util.py").contains(&"util_v1".to_string()),
        "premise: the cold walk indexes the link, got {:?}",
        symbols_of(&store, "src/util.py")
    );

    // The edit, and the watcher event it produces: the link's own path.
    std::fs::write(
        root.join("shared/util.py"),
        "def util_v2():\n    return 2\n",
    )
    .unwrap();
    store
        .enqueue_pending_paths_under_root(&root, &["src/util.py".to_string()])
        .unwrap();
    let reconciled = store.reconcile_pending_paths(&root).unwrap();
    assert!(
        reconciled.dropped.is_empty(),
        "the queued edit must survive reconcile: {:?}",
        reconciled.dropped
    );

    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    daemon
        .drain_pending_batch()
        .expect("the drain must succeed");

    assert!(
        symbols_of(&store, "src/util.py").contains(&"util_v2".to_string()),
        "the drain must read the link's bytes, as the cold walk does; got {:?}",
        symbols_of(&store, "src/util.py")
    );
    assert!(
        matches!(
            store.latest_analysis().unwrap().unwrap().status,
            AnalysisStatus::Ok
        ),
        "nothing was refused in this repository, so nothing may be charged"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// (c), the refusing half: a source that becomes a link out of the repository
/// is refused by the cold walk, so a `devmap build` writes no rows for it. The
/// drain has to reach the same state — and say so — instead of failing the
/// batch until the path quarantines while the rows stay.
#[test]
fn a_drain_of_an_escaping_symlink_drops_its_rows_and_charges_the_refusal() {
    let root = scratch("escaping");
    let outside = root
        .parent()
        .unwrap()
        .join(format!("devmap-symlink-e2e-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.py"), "def leaked():\n    return 1\n").unwrap();

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    std::fs::write(root.join("src/creds.py"), "def creds():\n    return 1\n").unwrap();
    let db_path = root.join("index.sqlite");

    let store = cold_build(&root, &db_path);
    assert!(
        symbols_of(&store, "src/creds.py").contains(&"creds".to_string()),
        "premise: it starts as an ordinary indexed source"
    );

    // Replaced by a link out of the repository — what a `ln -sf` into a shared
    // directory does, and what the cold walk refuses.
    std::fs::remove_file(root.join("src/creds.py")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.py"), root.join("src/creds.py")).unwrap();
    let (cold_sources, cold_report) = devmap_extract::collect_sources_with_report(&root).unwrap();
    assert!(
        !cold_sources.iter().any(|(path, _)| path == "src/creds.py")
            && cold_report.refused_count() == 1,
        "premise: the cold walk now refuses it, so a full build writes no rows \
         for it and records one refusal"
    );

    store
        .enqueue_pending_paths(&[root.join("src/creds.py").to_string_lossy().into_owned()])
        .unwrap();
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    daemon
        .drain_pending_batch()
        .expect("a refused path is a fact about coverage, not a batch failure");

    assert!(
        symbols_of(&store, "src/creds.py").is_empty(),
        "the graph must stop claiming symbols the build path stopped writing: {:?}",
        symbols_of(&store, "src/creds.py")
    );
    assert!(
        symbols_of(&store, "src/a.py").contains(&"a".to_string()),
        "the rest of the repository is untouched"
    );
    let after = store.latest_analysis().unwrap().unwrap().status;
    assert!(
        !matches!(after, AnalysisStatus::Ok),
        "a file this drain refused is a file whose calls were never read; \
         reporting complete coverage of it is the check that could not run \
         answering like one that ran. Got {after:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// The drain must not descend a directory link either, or the same bytes land
/// in the graph twice — once under the real path the walk found, once under the
/// link the walk never followed.
#[test]
fn a_drain_of_a_directory_symlink_does_not_index_the_target_twice() {
    let root = scratch("dirlink");
    std::fs::create_dir_all(root.join("shared/pkg")).unwrap();
    std::fs::write(root.join("shared/pkg/mod.py"), "def m():\n    return 1\n").unwrap();
    std::os::unix::fs::symlink(root.join("shared/pkg"), root.join("pkg")).unwrap();
    let db_path = root.join("index.sqlite");

    let store = cold_build(&root, &db_path);
    assert!(
        symbols_of(&store, "shared/pkg/mod.py").contains(&"m".to_string()),
        "premise: the real path is indexed"
    );

    store
        .enqueue_pending_paths(&[root.join("pkg").to_string_lossy().into_owned()])
        .unwrap();
    let daemon = Daemon::new(Store::open(&db_path).unwrap(), root.clone());
    daemon
        .drain_pending_batch()
        .expect("the drain must succeed");

    let indexed: Vec<String> = store
        .latest_extractions()
        .unwrap()
        .into_iter()
        .map(|extraction| extraction.file_path)
        .collect();
    assert!(
        !indexed.iter().any(|path| path.starts_with("pkg/")),
        "the drain descended a link the walk does not: {indexed:?}"
    );
    assert!(
        indexed.contains(&"shared/pkg/mod.py".to_string()),
        "and it must not have lost the real path: {indexed:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
