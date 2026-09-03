//! `preview` must not read files outside the indexed repository.
//!
//! The IPC surface takes `file` straight from the caller. Joined onto the
//! repository root with no traversal check — and used verbatim when absolute —
//! `{"cmd":"preview","file":"../other/secrets.py","content":""}` answers with
//! every symbol name and signature of any file the daemon's uid can read. The
//! containment rule is the one `daemon.rs::collect_pending_path` already
//! applies to watcher paths.

use devmap_extract::extract_file;
use devmap_query::StoreQueryEngine;
use devmap_query::PREVIEW_CALLER_MIN_CONFIDENCE;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

/// A scratch directory named for the test using it.
///
/// No `tempfile` dev-dependency exists in this workspace and this does not add
/// one; the convention here is a process-scoped path under the system temp dir,
/// with the test name in it because these run in parallel in one process.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("devmap-preview-esc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`); the recorded
    // repo root is always canonical, so the fixture's must be too.
    dir.canonicalize().unwrap()
}

const LIB: &str = "def compute(rows):\n    return sum(rows)\n";

/// Build a store rooted at `root` holding one repository-relative file.
fn fixture(root: &Path) -> Store {
    std::fs::write(root.join("lib.py"), LIB).unwrap();
    let extractions = vec![extract_file("lib.py", LIB)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store
}

/// A relative path that walks out of the repository is refused, not read.
#[test]
fn a_parent_traversal_is_refused() {
    let dir = scratch("parent");
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        dir.join("secrets.py"),
        "def api_key():\n    return 'hunter2'\n",
    )
    .unwrap();
    let store = fixture(&root);

    let error = StoreQueryEngine::new(&store)
        .preview("../secrets.py", "", 2_000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .expect_err("a path escaping the repository root must be refused");
    let message = error.to_string();
    assert!(
        message.contains("outside") || message.contains("traversal"),
        "the refusal must name the containment rule, got: {message}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An absolute path outside the repository is refused rather than read verbatim.
#[test]
fn an_absolute_path_outside_the_root_is_refused() {
    let dir = scratch("absolute");
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let outside = dir.join("secrets.py");
    std::fs::write(&outside, "def api_key():\n    return 'hunter2'\n").unwrap();
    let store = fixture(&root);

    let error = StoreQueryEngine::new(&store)
        .preview(
            &outside.to_string_lossy(),
            "",
            2_000,
            PREVIEW_CALLER_MIN_CONFIDENCE,
        )
        .expect_err("an absolute path outside the repository must be refused");
    assert!(
        error.to_string().contains("outside"),
        "the refusal must say the path is outside the root, got: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A symlink that points out of the repository is refused.
///
/// Component-level checks pass this: every component sits under the root, and
/// only resolving the link shows where the read would actually land.
#[test]
#[cfg(unix)]
fn a_symlink_escaping_the_root_is_refused() {
    let dir = scratch("symlink");
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let outside = dir.join("secrets.py");
    std::fs::write(&outside, "def api_key():\n    return 'hunter2'\n").unwrap();
    let store = fixture(&root);
    std::os::unix::fs::symlink(&outside, root.join("link.py")).unwrap();

    let error = StoreQueryEngine::new(&store)
        .preview("link.py", "", 2_000, PREVIEW_CALLER_MIN_CONFIDENCE)
        .expect_err("a symlink resolving outside the repository must be refused");
    assert!(
        error.to_string().contains("outside"),
        "the refusal must say the path resolves outside the root, got: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The guard must refuse escapes without refusing the ordinary case: a file
/// inside the repository still previews, and still compares against disk.
#[test]
fn an_in_root_file_still_previews_against_disk() {
    let dir = scratch("inroot");
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let store = fixture(&root);

    let report = StoreQueryEngine::new(&store)
        .preview(
            "lib.py",
            "def compute(rows):\n    return sum(rows)\n\ndef added():\n    return 1\n",
            2_000,
            PREVIEW_CALLER_MIN_CONFIDENCE,
        )
        .expect("an in-root path must preview");
    assert!(report.delta_available);
    assert_eq!(
        report.compared_against, "disk",
        "the on-disk file must still be found through the containment check"
    );
    assert!(
        report
            .symbols
            .iter()
            .any(|symbol| symbol.symbol_name == "added"),
        "the added symbol must be reported: {:?}",
        report.symbols
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An absolute path *inside* the repository is the daemon's own convention for
/// watcher paths, and stays accepted — the rule is containment, not a blanket
/// ban on absolute paths.
#[test]
fn an_absolute_path_inside_the_root_is_accepted() {
    let dir = scratch("absin");
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let store = fixture(&root);

    let report = StoreQueryEngine::new(&store)
        .preview(
            &root.join("lib.py").to_string_lossy(),
            LIB,
            2_000,
            PREVIEW_CALLER_MIN_CONFIDENCE,
        )
        .expect("an absolute path inside the repository must preview");
    assert_eq!(report.compared_against, "disk");
    let _ = std::fs::remove_dir_all(&dir);
}
