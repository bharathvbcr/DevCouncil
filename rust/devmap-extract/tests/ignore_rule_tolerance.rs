//! K-A1: one malformed glob in a `.gitignore` must not freeze ignore evaluation.
//!
//! `GitignoreBuilder::add` returns a *partial* error — the offending line is
//! dropped and every other line is still compiled (verified in
//! `ignore-0.4.33/src/gitignore.rs:405-434`, which accumulates into a
//! `PartialErrorBuilder` and never breaks out of the loop for a bad glob).
//! Treating that return as fatal makes `is_gitignored` fail for *every* path
//! under the tree, and the watcher then reads the failure as "ignore this
//! path" — a check that could not run producing the same value as a check that
//! ran and said no.
//!
//! The cold `devmap build` walker (`WalkBuilder`, `collect_sources_with_report`)
//! tolerates the same file. The doc comment on `is_gitignored` promises the two
//! agree; these tests hold it to that.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use devmap_extract::{collect_sources_with_report, is_gitignored};

/// `[z-a]` is a plausible typo (`git` itself tolerates it) that `ignore`
/// rejects: "error parsing glob '[z-a]': invalid range; 'z' > 'a'".
const MALFORMED_GLOB: &str = "[z-a]";

fn scratch_repo(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-ignore-tolerance-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // `WalkBuilder` honours `.gitignore` only inside a git worktree
    // (`require_git` defaults to true, `ignore-0.4.33/src/dir.rs:800`), and
    // `ignore_rule_bases` roots its stack at the same marker. Both halves must
    // see the same tree or the comparison below is meaningless.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

/// Every source file the cold walker actually yielded, repo-relative.
fn cold_build_files(root: &Path) -> BTreeSet<String> {
    let (sources, _report) = collect_sources_with_report(root).expect("cold discovery must run");
    sources.into_iter().map(|(path, _)| path).collect()
}

/// K-A1, the core defect: a single bad glob must not turn every verdict into an
/// error. The lines around it still have to be applied — dropping them would be
/// a different bug (nothing ignored) with the same test outcome if we only
/// asserted "does not error".
#[test]
fn a_malformed_glob_must_not_freeze_ignore_evaluation() {
    let root = scratch_repo("malformed");
    write(
        &root,
        ".gitignore",
        &format!("before.py\n{MALFORMED_GLOB}\nafter.py\n"),
    );
    write(&root, "before.py", "def before(): pass\n");
    write(&root, "after.py", "def after(): pass\n");
    write(&root, "src.py", "def src(): pass\n");

    let verdict = |relative: &str| {
        is_gitignored(&root, &root.join(relative), false).unwrap_or_else(|error| {
            panic!(
                "one malformed glob froze the ignore check for {relative}: {error}\n\
                 every path under this tree now fails the same way, and the watcher \
                 reads that failure as `ignored`"
            )
        })
    };

    assert!(
        verdict("before.py"),
        "a rule listed before the malformed glob must still be applied"
    );
    assert!(
        verdict("after.py"),
        "a rule listed after the malformed glob must still be applied; \
         `GitignoreBuilder::add` drops only the offending line"
    );
    assert!(
        !verdict("src.py"),
        "an unlisted source file must not be ignored — a fix that ignores \
         nothing is a different bug"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Positive control: nothing about the tolerance may change a well-formed file.
#[test]
fn a_well_formed_gitignore_still_ignores_exactly_what_it_names() {
    let root = scratch_repo("wellformed");
    write(&root, ".gitignore", "build/\nignored.py\n!kept.py\n");
    write(&root, "ignored.py", "x = 1\n");
    write(&root, "kept.py", "x = 1\n");
    write(&root, "src.py", "x = 1\n");
    write(&root, "build/generated.py", "x = 1\n");

    assert!(is_gitignored(&root, &root.join("ignored.py"), false).unwrap());
    assert!(is_gitignored(&root, &root.join("build"), true).unwrap());
    assert!(is_gitignored(&root, &root.join("build/generated.py"), false).unwrap());
    assert!(!is_gitignored(&root, &root.join("src.py"), false).unwrap());
    assert!(
        !is_gitignored(&root, &root.join("kept.py"), false).unwrap(),
        "a whitelist line must still win"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The doc comment on `is_gitignored` claims it evaluates rules "the same way
/// `WalkBuilder` does for a cold build". A malformed glob is exactly where that
/// claim was false: the cold walk indexed the tree and the watcher dropped all
/// of it. Compare the two directly rather than trusting the comment.
#[test]
fn the_cold_walker_and_the_incremental_check_agree_under_a_malformed_glob() {
    let root = scratch_repo("agreement");
    write(
        &root,
        ".gitignore",
        &format!("ignored.py\n{MALFORMED_GLOB}\nbuild/\n"),
    );
    write(&root, "ignored.py", "x = 1\n");
    write(&root, "src.py", "x = 1\n");
    write(&root, "pkg/mod.py", "x = 1\n");
    write(&root, "build/generated.py", "x = 1\n");

    let indexed = cold_build_files(&root);
    assert!(
        indexed.contains("src.py"),
        "the cold walker tolerates the malformed glob and still indexes the \
         tree: {indexed:?}"
    );

    for candidate in ["ignored.py", "src.py", "pkg/mod.py", "build/generated.py"] {
        let ignored = is_gitignored(&root, &root.join(candidate), false).unwrap_or_else(|error| {
            panic!("incremental ignore check failed for {candidate}: {error}")
        });
        assert_eq!(
            ignored,
            !indexed.contains(candidate),
            "cold build and incremental check disagree about {candidate}: \
             cold build indexed={}, incremental ignored={ignored}",
            indexed.contains(candidate)
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// A `.gitignore` that cannot be opened at all is still not a licence to call
/// every path ignored: the rules simply do not apply, which is what the cold
/// walker does with the same file.
#[test]
fn an_unreadable_ignore_file_leaves_paths_admitted_rather_than_ignored() {
    let root = scratch_repo("unreadable");
    write(&root, ".gitignore", "src.py\n");
    write(&root, "src.py", "x = 1\n");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let rules = root.join(".gitignore");
        std::fs::set_permissions(&rules, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_to_string(&rules).is_ok() {
            // Running as root: the mode is advisory and the file is still
            // readable, so this case cannot be exercised here.
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let ignored = is_gitignored(&root, &root.join("src.py"), false)
            .expect("an unreadable rule file must not turn every verdict into an error");
        assert!(
            !ignored,
            "rules that could not be read must not be reported as matching"
        );
        std::fs::set_permissions(&rules, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// K-A1's other half: the dropped line is *reported*, not silently swallowed.
///
/// Tolerating the malformed glob is only half an honest answer. A rule the
/// developer wrote and the kernel is not applying is invisible from both sides
/// — the watcher sees a path it admits, the developer sees a `.gitignore` line
/// that does nothing — so the verdict travels with one diagnostic per line that
/// could not be compiled. Nothing else in the workspace asserts this, and the
/// `problems.push` in `add_ignore_rules` can be deleted without a single
/// existing test failing.
#[test]
fn the_unusable_rule_line_is_reported_alongside_the_verdict() {
    let root = scratch_repo("reporting");
    write(
        &root,
        ".gitignore",
        &format!("before.py\n{MALFORMED_GLOB}\nafter.py\n"),
    );
    write(&root, "before.py", "def before(): pass\n");
    write(&root, "src.py", "def src(): pass\n");

    let (ignored, problems) =
        devmap_extract::is_gitignored_reporting(&root, &root.join("src.py"), false)
            .expect("a partly-unusable rule file must still produce a verdict");
    assert!(!ignored, "the verdict itself is unchanged");
    assert_eq!(
        problems.len(),
        1,
        "exactly one rule file was partly unusable: {problems:?}"
    );
    assert!(
        problems[0].contains(".gitignore"),
        "the diagnostic must name the file whose lines are not being applied: {problems:?}"
    );
    assert!(
        problems[0].contains("not being applied"),
        "and must say the lines are not in force, not merely that something was odd: \
         {problems:?}"
    );

    // Positive control: a well-formed tree pays nothing and reports nothing, so
    // a caller can treat a non-empty list as real news.
    let clean = scratch_repo("reporting-clean");
    write(&clean, ".gitignore", "build/\n");
    write(&clean, "src.py", "def src(): pass\n");
    let (_, none) = devmap_extract::is_gitignored_reporting(&clean, &clean.join("src.py"), false)
        .expect("a well-formed rule file must produce a verdict");
    assert!(
        none.is_empty(),
        "a well-formed tree must report no problems: {none:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&clean);
}

#[test]
fn relative_gitfile_pointers_and_submodule_excludes_share_the_same_owner() {
    let root = scratch_repo("relative-gitfile");
    let metadata = root.join("private-metadata");
    std::fs::rename(root.join(".git"), &metadata).unwrap();
    std::fs::create_dir(metadata.join("info")).unwrap();
    std::fs::write(metadata.join("info/exclude"), "hidden.py\n").unwrap();
    std::fs::write(root.join(".git"), "gitdir: private-metadata\n").unwrap();
    std::fs::write(root.join("hidden.py"), "pass\n").unwrap();
    let resolved = devmap_extract::git_metadata(&root).unwrap().unwrap();
    assert_eq!(resolved.git_dir, metadata.canonicalize().unwrap());
    assert_eq!(resolved.common_dir, resolved.git_dir);
    assert!(is_gitignored(&root, &root.join("hidden.py"), false).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_or_oversized_git_pointers_are_errors_not_non_git_trees() {
    let root = scratch_repo("bad-gitfile");
    std::fs::remove_dir(root.join(".git")).unwrap();
    for text in [
        "".to_string(),
        "gitdir: \n".to_string(),
        "garbage\n".to_string(),
        "x".repeat(65_537),
    ] {
        std::fs::write(root.join(".git"), text).unwrap();
        assert!(devmap_extract::git_metadata(&root).is_err());
    }
    std::fs::remove_file(root.join(".git")).unwrap();
    assert!(devmap_extract::git_metadata(&root).unwrap().is_none());
    std::fs::remove_dir_all(root).unwrap();
}
