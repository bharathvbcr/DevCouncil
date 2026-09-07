//! One directory the walker cannot open must not cost the whole map.
//!
//! Discovery already has the right answer for a file it cannot read: it is
//! recorded as `DiscoverySkipReason::Unreadable`, `is_refusal` counts it as
//! coverage loss, and the other few thousand files are indexed. A *directory*
//! it cannot open took the opposite path — `for result in walker { let entry =
//! result?; }` propagated the walker's error out of `collect_sources_with_report`
//! and the build stopped.
//!
//! Measured with the release binary on a tree holding one `chmod 000`
//! directory beside eight ordinary sources:
//!
//! ```text
//! $ devmap build <root>            # directory mode 000
//!   {"error":"…/noperm: IO error … Permission denied (os error 13)"}   exit 1
//! $ devmap --json status
//!   {"is_fresh": false, "node_count": 0,
//!    "degraded_reason": "this store holds no generation: nothing has been indexed yet"}
//!
//! $ chmod 755 <root>/noperm && devmap build <root>
//!   {"files_indexed": 5, "discovery_refused_files": 3, …}
//!     src/over_limit.py:  Oversized { bytes: 1048577, limit: 1048576 }
//!     src/unreadable.py:  Unreadable { reason: "Permission denied (os error 13)" }
//! ```
//!
//! Two spellings of "this indexer could not read that path", one recorded as
//! coverage loss and one fatal. The fatal one leaves an agent with no map at
//! all, which is the outcome every other refusal in this file exists to avoid:
//! a root-owned build directory, a `.git` object pack with odd modes, or a
//! mount that lost `+x` are ordinary, and none of them is a reason to refuse to
//! describe the rest of the repository.
//!
//! The root itself is different and stays fatal: a root that cannot be opened
//! has not been examined at all, and answering "zero sources, complete" for one
//! is precisely the check-that-could-not-run reporting as a check that passed.

use std::fs;
use std::path::PathBuf;

use devmap_extract::collect_sources_with_report;
use devmap_extract::model::DiscoverySkipReason;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-unreadable-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Restore the mode whatever the test does, so a failure cannot leave a
/// directory the harness cannot clean up.
struct Unreadable(PathBuf);

impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn a_directory_the_walker_cannot_open_is_a_refusal_and_the_rest_is_indexed() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("dir");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("locked")).unwrap();
    for index in 0..8 {
        fs::write(
            root.join(format!("src/mod_{index}.py")),
            format!("def fn_{index}():\n    return {index}\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("locked/inside.py"),
        "def hidden():\n    return 1\n",
    )
    .unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(root.join("locked"));

    let (sources, report) = match collect_sources_with_report(&root) {
        Ok(answer) => answer,
        Err(error) => panic!(
            "one unreadable directory ended the whole discovery pass with {error}; \
             the eight readable sources beside it were never indexed, and an \
             unreadable *file* in the same tree is recorded as a refusal and \
             skipped"
        ),
    };

    assert_eq!(
        sources.len(),
        8,
        "every readable source must still be collected, got {:?}",
        sources.iter().map(|(path, _)| path).collect::<Vec<_>>()
    );

    // Class A: skipping it silently would be worse than failing. The path has
    // to be named, and it has to count as loss.
    let refusal = report
        .skipped_paths
        .iter()
        .find(|(path, _)| path.starts_with("locked"))
        .unwrap_or_else(|| {
            panic!(
                "the unreadable directory must be recorded, not passed over: {:?}",
                report.skipped_paths
            )
        });
    assert!(
        matches!(refusal.1, DiscoverySkipReason::Unreadable { .. }),
        "an unopenable directory is unreadable, not something else: {refusal:?}"
    );
    assert!(
        refusal.1.is_refusal(),
        "a subtree nobody could read is a hole in the graph"
    );
    assert_eq!(
        report.refusals().count(),
        1,
        "exactly the unreadable directory, once: {:?}",
        report.skipped_paths
    );
}

/// The OFF direction. A root that cannot be opened has not been examined, and
/// "zero sources" is not the answer to that.
#[test]
fn a_root_that_cannot_be_opened_is_still_an_error() {
    let missing = scratch("root").join("does-not-exist");
    let error = collect_sources_with_report(&missing)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| {
            panic!(
                "a missing root answered instead of failing, so an empty index \
                    reads as a repository with nothing in it"
            )
        });
    assert!(
        error.contains("does-not-exist"),
        "the failure must name the root it could not open: {error}"
    );

    use std::os::unix::fs::PermissionsExt;
    let locked = scratch("lockedroot");
    fs::write(locked.join("a.py"), "def a():\n    return 1\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(locked.clone());
    assert!(
        collect_sources_with_report(&locked).is_err(),
        "a root this process cannot open must fail rather than report an empty \
         repository"
    );
}

/// A root that exists and is not a directory was never examined either.
///
/// `ignore::WalkBuilder` walks a file root by yielding the file itself, so a
/// build pointed at a regular file "ran": zero candidates, zero sources, and a
/// generation that reads as a repository with nothing in it. Measured through
/// the release binary: `devmap build <regular file>` exited 0 with
/// `files_indexed: 0` and wrote a generation, while `devmap build <missing
/// path>` exited 1 — two roots the walk could not examine, one a dead build
/// and one a passing one.
#[test]
fn a_root_that_is_not_a_directory_is_refused_not_walked_as_an_empty_tree() {
    let dir = scratch("fileroot");
    let file = dir.join("README.md");
    fs::write(&file, "# not a repository\n").unwrap();
    let error = collect_sources_with_report(&file)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| {
            panic!(
                "a regular file answered as a root instead of failing, so an empty \
                 index reads as a repository with nothing in it"
            )
        });
    assert!(
        error.contains("README.md") && error.contains("not a directory"),
        "the failure must name the root and say what is wrong with it: {error}"
    );
}

/// The same `result?`, in the second walk of the same tree.
///
/// `devmap build` runs `collect_sources_with_report` and then
/// `collect_go_modules(path)?` over the same root, so fixing only the first
/// left the build failing exactly as before — measured through the release
/// binary, which still exited 1 on the `chmod 000` directory after discovery
/// had already recorded its two file refusals and moved on.
///
/// Go module discovery is enrichment: a directory it cannot open holds no
/// `go.mod` it can read, and that is not a reason to refuse to describe the
/// repository. The loss is disclosed by `collect_sources_with_report`, which
/// walks the same tree and records the same directory as a refusal.
#[test]
fn go_module_discovery_steps_over_a_subtree_it_cannot_open() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("gomod");
    fs::create_dir_all(root.join("svc")).unwrap();
    fs::create_dir_all(root.join("locked")).unwrap();
    fs::write(
        root.join("svc/go.mod"),
        "module example.com/svc\n\ngo 1.22\n",
    )
    .unwrap();
    fs::write(root.join("locked/go.mod"), "module example.com/hidden\n").unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = Unreadable(root.join("locked"));

    let modules = devmap_extract::collect_go_modules(&root).unwrap_or_else(|error| {
        panic!(
            "one unreadable directory ended Go module discovery with {error}, \
             which fails the whole `devmap build` that calls it"
        )
    });
    assert_eq!(
        modules.len(),
        1,
        "the readable module must still be found: {modules:?}"
    );
    assert_eq!(modules[0].prefix, "example.com/svc");
}

/// And a root it cannot open is still an error here too, for the same reason.
#[test]
fn go_module_discovery_still_fails_on_a_root_it_cannot_open() {
    let missing = scratch("gomodroot").join("does-not-exist");
    assert!(
        devmap_extract::collect_go_modules(&missing).is_err(),
        "a missing root must fail rather than report a repository with no Go modules"
    );
}
