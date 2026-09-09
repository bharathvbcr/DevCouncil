//! The staleness check and the walk that fills the index must cover one tree.
//!
//! Discovery honours the Cache Directory Tagging Standard: `collect_sources`
//! prunes any directory holding a `CACHEDIR.TAG` whose first 43 bytes are the
//! standard's signature, whole, without descending (K7). cargo, pip, uv,
//! ccache, tox, ruff and pytest all write one, and a build cache is not source.
//!
//! `freshness::inventory` did not. It ran `git ls-files --cached` plus
//! `git ls-files --others --exclude-standard` and filtered the result through
//! `is_runtime_or_generated_file`, a pure function of the path string that
//! cannot open a `CACHEDIR.TAG`. So a tagged cache directory that no
//! `.gitignore` happens to cover was skipped by the walk — never indexed — and
//! counted by the fingerprint, which then moved on every write into it.
//!
//! The map is permanently stale on files it deliberately never indexes:
//! `dev map --if-stale`, `--watch` and `verify` rebuild forever and never
//! converge, and each rebuild rewrites the stamp they will disagree with again.
//! Live example on this workspace at the time of writing: `rust-port/.gitignore`
//! carries `/target/`, which does not match `rust-port/target-laneA`, and
//! `git ls-files --others --exclude-standard` listed 58,428 paths under
//! `rust-port/target-lane*` — all of them tagged, none of them indexed.
//!
//! The two questions are answered by two different walks on purpose (git is far
//! faster than a filesystem walk for the inventory), so nothing makes them agree
//! automatically. This test is what makes them agree.

use std::path::{Path, PathBuf};
use std::process::Command;

use devmap_query::freshness::{content_fingerprint, inventory, InventoryLimits, InventorySource};

/// A repository with one real source file and one tagged cache directory, and
/// deliberately **no** `.gitignore`: an ignored cache is invisible to
/// `git ls-files` already, so it proves nothing. The bug lives in the gap
/// between "the tool said this is a cache" and "git was never told".
fn tagged_cache_repo(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-cachetag-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target-lane9/debug")).unwrap();
    std::fs::write(root.join("src/app.py"), b"def app():\n    return 1\n").unwrap();
    // The exact signature line cargo and friends write, plus the human comment
    // the standard recommends. Checked by signature rather than by filename, so
    // the bytes are load-bearing.
    std::fs::write(
        root.join("target-lane9/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n\
         # This file is a cache directory tag created by a build tool.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("target-lane9/debug/artifact.py"),
        b"def artifact():\n    return 2\n",
    )
    .unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "cachetag@example.invalid"]);
    git(&["config", "user.name", "cachetag"]);
    git(&["add", "src/app.py"]);
    git(&["commit", "-qm", "one real source file"]);
    root
}

/// Everything discovery would index, repo-relative.
fn discovered(root: &Path) -> Vec<String> {
    devmap_extract::collect_sources(root)
        .expect("the discovery walk runs")
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

#[test]
fn a_tagged_cache_directory_is_absent_from_the_inventory_because_it_is_absent_from_the_index() {
    let root = tagged_cache_repo("inventory");

    // The premise, stated rather than assumed: the cache is not gitignored, so
    // git lists it, and the walk skips it anyway.
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(&root)
        .output()
        .expect("git runs");
    let listed = String::from_utf8_lossy(&untracked.stdout);
    assert!(
        listed.contains("target-lane9/debug/artifact.py"),
        "premise: an unignored cache directory is listed by git: {listed:?}"
    );

    let walked = discovered(&root);
    assert!(
        walked.iter().any(|path| path == "src/app.py"),
        "premise: real sources are discovered: {walked:?}"
    );
    assert!(
        !walked.iter().any(|path| path.starts_with("target-lane9/")),
        "premise: discovery prunes a CACHEDIR.TAG directory whole: {walked:?}"
    );

    let listing = inventory(&root, InventoryLimits::default());
    assert_eq!(listing.source, InventorySource::Git);
    assert!(
        listing.files.iter().any(|path| path == "src/app.py"),
        "the real source must stay in the inventory: {:?}",
        listing.files
    );
    let counted: Vec<&String> = listing
        .files
        .iter()
        .filter(|path| path.starts_with("target-lane9/"))
        .collect();
    assert!(
        counted.is_empty(),
        "freshness counted {} path(s) inside a tagged cache directory that discovery \
         will never index, so the map can never stop being stale: {counted:?}",
        counted.len()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn writing_only_inside_a_tagged_cache_directory_does_not_move_the_content_fingerprint() {
    let root = tagged_cache_repo("fingerprint");

    // `persist_cache=false`: the memo is advisory, and a test must not leave one
    // behind that the next test reads as authoritative.
    let before = content_fingerprint(
        &root,
        &inventory(&root, InventoryLimits::default()).files,
        false,
    );

    // A build writes into its own cache. Nothing a `dev map` run would index has
    // changed, so nothing the map answers can have changed either.
    std::fs::write(
        root.join("target-lane9/debug/artifact.py"),
        b"def artifact():\n    return 3\n",
    )
    .unwrap();
    std::fs::write(root.join("target-lane9/debug/fresh.json"), b"{}\n").unwrap();

    let after = content_fingerprint(
        &root,
        &inventory(&root, InventoryLimits::default()).files,
        false,
    );
    assert_eq!(
        before, after,
        "a write no walk will ever read marked the map stale; every rebuild \
         re-stamps a fingerprint the next build will disagree with again"
    );

    // …and the check still has teeth: an edit to indexed source must move it.
    std::fs::write(root.join("src/app.py"), b"def app():\n    return 99\n").unwrap();
    let edited = content_fingerprint(
        &root,
        &inventory(&root, InventoryLimits::default()).files,
        false,
    );
    assert_ne!(
        before, edited,
        "skipping cache directories must not blind the fingerprint to real edits"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The second shape of the same disagreement, found by attacking the pair
/// rather than by re-reading the cache rule.
///
/// Discovery refuses a symlink whose target resolves outside the repository —
/// `preview` and `classify_pending_entry` have always refused such a path, and
/// `collect_sources_with_report` now does too, recording it as
/// `DiscoverySkipReason::EscapesRoot`. `git ls-files` lists the link (it is a
/// tracked mode-120000 entry), and `keep_indexable`'s final `is_file()` follows
/// it, so the inventory counted a file discovery will never index and hashed
/// bytes the map does not describe.
///
/// Measured with the release binary before the fix, on the fixture below:
///
/// ```text
/// devmap build      files_indexed: 1   discovery_refused_files: 1   (paths: a.py)
/// devmap freshness  files: 2           c2:496ee98036e5aaad…
/// # edit the file OUTSIDE the repository, through the in-tree symlink
/// devmap freshness  files: 2           c2:8bcb1a2b56f64d37…
/// ```
///
/// The direction is the same as the cache case and so is the cost: the map
/// reads stale on a file it deliberately never indexed, so `--if-stale`,
/// `--watch` and `verify` rebuild on a change they cannot absorb.
#[test]
fn a_symlink_out_of_the_repository_is_absent_from_the_inventory_too() {
    let base = std::env::temp_dir().join(format!(
        "devmap-escapelink-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = base.join("repo");
    let outside = base.join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("s.py"), "SECRET_V1 = 1\n").unwrap();
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("s.py"), root.join("esc.py")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(outside.join("s.py"), root.join("esc.py")).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "x",
        ],
    ] {
        let ok = Command::new("git")
            .args(&args)
            .current_dir(&root)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("skipped: git is unavailable here");
            let _ = std::fs::remove_dir_all(&base);
            return;
        }
    }

    // The premise, checked rather than assumed: git really does list the link,
    // so the inventory has something to wrongly keep.
    let listed = Command::new("git")
        .args(["ls-files"])
        .current_dir(&root)
        .output()
        .expect("git ls-files");
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.contains("esc.py"),
        "the fixture must have a tracked symlink for this to mean anything: {listed}"
    );

    let indexed = discovered(&root);
    assert!(
        !indexed.iter().any(|path| path == "esc.py"),
        "discovery must refuse the escaping symlink, or this test is about \
         something else: {indexed:?}"
    );

    let listing = inventory(&root, InventoryLimits::default());
    assert!(matches!(listing.source, InventorySource::Git));
    assert!(
        !listing.files.iter().any(|path| path == "esc.py"),
        "the inventory counted a file discovery refuses to index: {:?}",
        listing.files
    );

    // And the digest must not move when a file outside the repository does.
    let before = content_fingerprint(&root, &listing.files, false);
    std::fs::write(outside.join("s.py"), "SECRET_V2 = 2\n").unwrap();
    let after = content_fingerprint(&root, &listing.files, false);
    assert_eq!(
        before, after,
        "editing a file outside the repository moved this repository's content \
         fingerprint, so the map reads stale on a change it can never absorb"
    );

    let _ = std::fs::remove_dir_all(&base);
}
