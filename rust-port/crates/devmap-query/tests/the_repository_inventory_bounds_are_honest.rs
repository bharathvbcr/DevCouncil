//! What `inventory::scan` says about its own bounds has to be true in both
//! directions.
//!
//! The module's contract is that everything it does is bounded "and says so
//! when a bound bit": `walk_truncated` makes both published lists a lower bound
//! rather than the repository's set, and `refused_oversize` exists so that
//! "could not read" and "read and found nothing" are never the same answer.
//! Both are published — `manifest.rs:764` writes them into `repo_map.json` as
//! `inventory_walk_truncated` and `inventory_refused_oversize`, beside
//! `package_managers_computed: true` / `test_commands_computed: true`.
//!
//! These are the two cases where the claim and the code disagreed.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use devmap_query::inventory;

/// A scratch root, unique per process and call.
fn root(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "devmap-inventory-{tag}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// The depth cap reports truncation for directories the walk had already
/// decided never to enter.
///
/// `walk_markers` tests `depth + 1 > WALK_DEPTH_CAP` before it tests
/// `skip_dir`, so a `node_modules` or a `.git` sitting one level past the cap
/// sets `walk_truncated` — even though the walk would have refused to descend
/// into it at any depth, and nothing a marker could live in was skipped. The
/// flag then tells every reader of `repo_map.json` that `package_managers` and
/// `test_commands` are a lower bound, on the evidence of a directory whose
/// contents are not evidence.
#[test]
fn a_skipped_directory_past_the_depth_cap_is_not_a_truncated_walk() {
    let root = root("depth");
    // `deep` sits at exactly `WALK_DEPTH_CAP`; its children are the first level
    // the cap refuses.
    let mut deep = root.clone();
    for level in 1..=inventory::WALK_DEPTH_CAP {
        deep = deep.join(format!("level{level}"));
    }
    // Both are `skip_dir` at every depth, so neither would have been entered
    // even with no cap at all.
    std::fs::create_dir_all(deep.join("node_modules")).unwrap();
    std::fs::create_dir_all(deep.join(".git")).unwrap();
    std::fs::write(root.join("uv.lock"), "").unwrap();
    std::fs::write(root.join("pyproject.toml"), "[project]\nname = 'x'\n").unwrap();

    let scanned = inventory::scan(&root);
    assert!(scanned.computed);
    assert_eq!(scanned.package_managers, vec!["uv".to_string()]);
    assert!(
        !scanned.walk_truncated,
        "the only directories past the cap are ones `skip_dir` refuses at every \
         depth; nothing that could hold a marker was cut, and reporting the \
         lists as a lower bound on that evidence is a bound that did not bite"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// …and a directory past the cap that the walk *would* have entered still is.
///
/// The companion: "never truncated" would pass the case above and lose the
/// signal entirely.
#[test]
fn a_real_directory_past_the_depth_cap_is_a_truncated_walk() {
    let root = root("depth-real");
    let mut deep = root.clone();
    for level in 1..=inventory::WALK_DEPTH_CAP {
        deep = deep.join(format!("level{level}"));
    }
    std::fs::create_dir_all(deep.join("src")).unwrap();
    std::fs::write(root.join("uv.lock"), "").unwrap();

    let scanned = inventory::scan(&root);
    assert!(
        scanned.walk_truncated,
        "a `src` one level past the cap could have held a `Cargo.toml` or a \
         `go.mod`; the walk stopped short and the lists are a lower bound"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The directory cap stops the walk and says so, and it stops at the number it
/// promises.
///
/// The bound exists so a generated fixture corpus or a checked-in dependency
/// cache the skip list does not name cannot turn an artifact write into a
/// full-disk traversal, and `inventory_directories_visited` is published so a
/// reader can size the walk that produced the answer.
#[test]
fn the_directory_cap_stops_the_walk_and_reports_both_numbers() {
    let root = root("dircap");
    // One past the cap, counting the root itself as the first visit.
    for index in 0..inventory::WALK_DIR_CAP {
        std::fs::create_dir(root.join(format!("d{index:06}"))).unwrap();
    }
    std::fs::write(root.join("uv.lock"), "").unwrap();

    let scanned = inventory::scan(&root);
    assert!(scanned.computed);
    assert_eq!(
        scanned.directories_visited,
        inventory::WALK_DIR_CAP,
        "the walk opened more directories than it promises to"
    );
    assert!(
        scanned.walk_truncated,
        "the tree was not exhausted; both lists are a lower bound and must say so"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A symlinked directory loop is not descended, so the depth cap is not what
/// has to save the walk from it.
///
/// `entry.file_type()` does not follow links, and a linked tree is reachable
/// from wherever it really lives — so the walk declines it rather than relying
/// on `WALK_DEPTH_CAP` to end a cycle it should never have entered.
#[test]
fn a_symlinked_directory_loop_is_not_walked() {
    let root = root("symloop");
    std::fs::create_dir(root.join("real")).unwrap();
    std::os::unix::fs::symlink(&root, root.join("real").join("back")).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
    std::fs::write(root.join("uv.lock"), "").unwrap();

    let started = std::time::Instant::now();
    let scanned = inventory::scan(&root);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the loop was followed: {:?}",
        started.elapsed()
    );
    assert_eq!(scanned.package_managers, vec!["uv".to_string()]);
    assert!(
        scanned.directories_visited <= 3,
        "root and `real` are the only real directories: {}",
        scanned.directories_visited
    );
    assert!(
        !scanned.walk_truncated,
        "declining a link is not running out of budget"
    );
    // Break the cycle before the sweep.
    let _ = std::fs::remove_file(root.join("real").join("back"));
    let _ = std::fs::remove_dir_all(root);
}

/// A manifest that exists and could not be read is not a manifest that said
/// nothing.
///
/// `read_bounded` refuses a manifest past `MANIFEST_READ_CAP` by name, and
/// returns a bare `None` for every other failure — a permission denial, a
/// document that is not UTF-8, a file that changed type under it. The caller
/// cannot tell that `None` from "the file is absent", so the scan reports
/// `test_commands` as computed with the manifest's contents contributing
/// nothing and `refused_oversize` empty. The existence of the file still
/// contributes its package manager, so the artifact says npm *and* says the
/// repository declares no scripts — which is exactly the answer a repository
/// with an empty `scripts` block gives.
#[test]
fn a_manifest_that_could_not_be_read_is_named_not_dropped() {
    use std::os::unix::fs::PermissionsExt;
    let root = root("unreadable");
    std::fs::write(
        root.join("package.json"),
        r#"{"scripts":{"test":"jest","lint":"eslint ."}}"#,
    )
    .unwrap();
    std::fs::set_permissions(
        root.join("package.json"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();

    let scanned = inventory::scan(&root);
    assert!(scanned.computed);
    assert_eq!(
        scanned.package_managers,
        vec!["npm".to_string()],
        "the file's existence is still evidence of the manager"
    );
    assert!(
        scanned.test_commands.is_empty(),
        "nothing could be read, so nothing can be claimed: {:?}",
        scanned.test_commands
    );
    assert!(
        scanned
            .unreadable
            .iter()
            .any(|entry| entry.starts_with("package.json")),
        "the manifest was there and its contents did not reach the answer; a \
         reader that is told only `test_commands: []` cannot tell that from a \
         repository that declares no scripts. named: {:?}",
        scanned.unreadable
    );
    // Restore so the temp sweep can remove it.
    let _ = std::fs::set_permissions(
        root.join("package.json"),
        std::fs::Permissions::from_mode(0o644),
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The size cap keeps its own name, and a manifest read whole names neither.
#[test]
fn an_oversize_manifest_stays_oversize_and_a_readable_one_is_named_nowhere() {
    let root = root("oversize");
    let big = "x".repeat(usize::try_from(inventory::MANIFEST_READ_CAP).unwrap() + 1);
    std::fs::write(root.join("Makefile"), &big).unwrap();
    std::fs::write(root.join("package.json"), r#"{"scripts":{"test":"jest"}}"#).unwrap();

    let scanned = inventory::scan(&root);
    assert_eq!(scanned.refused_oversize, vec!["Makefile".to_string()]);
    assert!(
        scanned.unreadable.is_empty(),
        "an oversize file is refused by size, not unreadable: {:?}",
        scanned.unreadable
    );
    assert_eq!(scanned.test_commands, vec!["npm test".to_string()]);
    let _ = std::fs::remove_dir_all(Path::new(&root));
}

/// A Makefile that only *mentions* a `test` target does not declare one.
///
/// `declares_test_target` asked whether a line starts in column 0 and has the
/// word `test` before its first colon. A comment starts in column 0 and a
/// comment about a removed target has both — so a repository whose Makefile
/// says `# test: dropped, use pytest` was published as declaring `make test`,
/// under `test_commands_computed: true`. The whole contract of this artifact is
/// that a value is evidence, and an agent reading it runs the command.
#[test]
fn a_makefile_comment_about_test_does_not_declare_make_test() {
    let root = root("makefile-comment");
    std::fs::write(
        root.join("Makefile"),
        "# test: dropped in 2024, use pytest directly\n\
         VERBOSE := 1\n\
         all:\n\t@echo building\n",
    )
    .unwrap();

    let scanned = inventory::scan(&root);
    assert!(scanned.computed);
    assert!(
        !scanned.test_commands.iter().any(|c| c == "make test"),
        "the Makefile declares no `test` target; publishing one tells an agent \
         to run a command that does not exist: {:?}",
        scanned.test_commands
    );
    let _ = std::fs::remove_dir_all(root);
}

/// …and one that really declares it still does.
#[test]
fn a_makefile_that_declares_test_still_names_make_test() {
    let root = root("makefile-real");
    std::fs::write(
        root.join("Makefile"),
        "# the test target lives below\n\
         all:\n\t@echo building\n\
         test: all\n\tpytest -q\n",
    )
    .unwrap();

    let scanned = inventory::scan(&root);
    assert_eq!(scanned.test_commands, vec!["make test".to_string()]);
    let _ = std::fs::remove_dir_all(root);
}
