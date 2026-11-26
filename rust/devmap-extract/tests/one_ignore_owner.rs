//! One owner for "is this path excluded": the watcher's per-path verdict
//! (`is_gitignored`) must equal what the cold walk (`collect_sources_with_report`,
//! which `status` and `build` both walk with) yields, for every rule source the
//! walk reads.
//!
//! The audit's reproduction (2026-10-09, `ts2`): `.ignore` listed `skipme.ts`.
//! The cold walk excluded it — `WalkBuilder` honours `.ignore` by default — and
//! the watcher, which read only `info/exclude` and the `.gitignore` chain,
//! admitted it. `search skipme` found it after a drain and lost it after a
//! build, forever, and `status` never settled.
//!
//! Written as a comparison rather than as expected verdicts, so the test cannot
//! drift from the walk: whatever `walk_builder` decides, the watcher must
//! decide too. Each case also states the verdict it expects, so a comparison
//! that passes because both sides ignore nothing still fails.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use devmap_extract::{collect_sources_with_report, ignore_rule_files, is_gitignored};

fn scratch(tag: &str, git: bool) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-one-ignore-owner-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    if git {
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
    }
    root.canonicalize().unwrap()
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn walked(root: &Path) -> BTreeSet<String> {
    let (sources, _) = collect_sources_with_report(root).expect("cold discovery must run");
    sources.into_iter().map(|(path, _)| path).collect()
}

/// Assert the watcher and the walk agree on `candidate`, and that the agreed
/// verdict is `expect_excluded`.
fn agree(root: &Path, candidate: &str, expect_excluded: bool, case: &str) {
    let walk_excluded = !walked(root).contains(candidate);
    let watcher_excluded = is_gitignored(root, &root.join(candidate), false)
        .unwrap_or_else(|error| panic!("{case}: verdict for {candidate} failed: {error}"));
    assert_eq!(
        watcher_excluded, walk_excluded,
        "{case}: the watcher says excluded={watcher_excluded} for {candidate} and the cold \
         walk says excluded={walk_excluded} — two answers to one question"
    );
    assert_eq!(
        walk_excluded, expect_excluded,
        "{case}: {candidate} expected excluded={expect_excluded}"
    );
}

/// The matrix of rule sources the walk reads, each against the watcher.
#[test]
fn the_watcher_and_the_walk_exclude_the_same_paths() {
    // `.ignore` at the root: the audit's case.
    let root = scratch("ignore-root", true);
    write(&root, ".ignore", "skipme.ts\n");
    write(&root, "src/skipme.ts", "export const skip = 1;\n");
    write(&root, "src/keep.ts", "export const keep = 1;\n");
    agree(&root, "src/skipme.ts", true, ".ignore at the root");
    agree(&root, "src/keep.ts", false, ".ignore at the root");

    // `.ignore` in a subdirectory governs only beneath it.
    let root = scratch("ignore-nested", true);
    write(&root, "a/.ignore", "x.ts\n");
    write(&root, "a/x.ts", "export const a = 1;\n");
    write(&root, "b/x.ts", "export const b = 1;\n");
    agree(&root, "a/x.ts", true, "nested .ignore");
    agree(&root, "b/x.ts", false, "nested .ignore");

    // `.ignore` outranks `.gitignore`, even a deeper one: a whitelist in
    // `.ignore` re-admits what `.gitignore` excludes.
    let root = scratch("ignore-beats-gitignore", true);
    write(&root, ".ignore", "!keep.ts\n");
    write(&root, "src/.gitignore", "*.ts\n");
    write(&root, "src/keep.ts", "export const keep = 1;\n");
    write(&root, "src/drop.ts", "export const drop = 1;\n");
    agree(
        &root,
        "src/keep.ts",
        false,
        ".ignore whitelist over .gitignore",
    );
    agree(
        &root,
        "src/drop.ts",
        true,
        ".ignore whitelist over .gitignore",
    );

    // `info/exclude`.
    let root = scratch("exclude", true);
    write(&root, ".git/info/exclude", "local.ts\n");
    write(&root, "local.ts", "export const local = 1;\n");
    agree(&root, "local.ts", true, "info/exclude");

    // A directory excluded by `.ignore` takes everything under it, even a file
    // a deeper rule would have admitted.
    let root = scratch("ignored-directory", true);
    write(&root, ".ignore", "generated/\n");
    write(&root, "generated/.gitignore", "!client.ts\n");
    write(&root, "generated/client.ts", "export const client = 1;\n");
    agree(
        &root,
        "generated/client.ts",
        true,
        "an excluded directory prunes",
    );

    // Outside a git repository, `.ignore` still applies and `.gitignore` does
    // not — `require_git`, on both sides.
    let root = scratch("no-git", false);
    write(&root, ".ignore", "a.ts\n");
    write(&root, ".gitignore", "b.ts\n");
    write(&root, "a.ts", "export const a = 1;\n");
    write(&root, "b.ts", "export const b = 1;\n");
    agree(&root, "a.ts", true, "no repository: .ignore");
    agree(&root, "b.ts", false, "no repository: .gitignore is git's");
}

/// Every `.ts` file under `root`, found by reading the filesystem and
/// consulting nothing that ignores: the population both verdicts are asked
/// about. `.git` and `.jj` are skipped only because they hold no sources.
fn every_ts_file(root: &Path) -> Vec<String> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != ".git" && name != ".jj" {
                    visit(root, &path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "ts") {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    visit(root, root, &mut out);
    out.sort();
    out
}

/// For every file in the tree, in both directions: the watcher excludes it
/// exactly when the cold walk does not yield it. Returns the excluded set so a
/// caller can pin the verdicts it expects (a comparison that passes because
/// both sides exclude everything would otherwise pass).
fn watcher_equals_walk(root: &Path, case: &str) -> BTreeSet<String> {
    let walked = walked(root);
    let mut excluded = BTreeSet::new();
    for candidate in every_ts_file(root) {
        let watcher_excluded = is_gitignored(root, &root.join(&candidate), false)
            .unwrap_or_else(|error| panic!("{case}: verdict for {candidate} failed: {error}"));
        assert_eq!(
            watcher_excluded,
            !walked.contains(&candidate),
            "{case}: the watcher says excluded={watcher_excluded} for {candidate}; the cold walk \
             yields it={} — two answers to one question",
            walked.contains(&candidate)
        );
        if watcher_excluded {
            excluded.insert(candidate);
        }
    }
    excluded
}

/// A `.git` file that points at a git directory, as a submodule or a linked
/// worktree has.
fn write_git_pointer(root: &Path, relative: &str, target: &str) {
    write(root, relative, &format!("gitdir: {target}\n"));
}

/// The version-control boundary is one rule written in the `ignore` crate and
/// read by the walk; the watcher must read it the same way inside a nested
/// clone, a submodule, a nested `.jj` and across a `.git` pointer that points
/// nowhere. The parent's `.gitignore` and `info/exclude` stop at the nested
/// repository; its own do not; `.ignore` crosses every boundary.
#[test]
fn the_watcher_and_the_walk_agree_across_nested_repositories() {
    let root = scratch("nested-boundaries", true);
    write(&root, ".gitignore", "*.gen.ts\nparent_only.ts\n");
    write(&root, ".git/info/exclude", "root_exclude.ts\n");
    write(&root, ".ignore", "everywhere.ts\n");
    for name in [
        "top.gen.ts",
        "parent_only.ts",
        "root_exclude.ts",
        "everywhere.ts",
        "keep.ts",
    ] {
        write(&root, name, "export const v = 1;\n");
    }

    // (b) a nested clone with its own .gitignore and info/exclude.
    write(&root, "clone/.git/info/exclude", "clone_exclude.ts\n");
    write(&root, "clone/.gitignore", "clone_only.ts\n");
    for name in [
        "x.gen.ts",
        "parent_only.ts",
        "root_exclude.ts",
        "everywhere.ts",
        "clone_only.ts",
        "clone_exclude.ts",
        "keep.ts",
        "sub/x.gen.ts",
        "sub/clone_only.ts",
    ] {
        write(&root, &format!("clone/{name}"), "export const v = 1;\n");
    }

    // (a) a submodule: a .git pointer to a real git directory outside it.
    write(&root, ".git/modules/sub/info/exclude", "sub_exclude.ts\n");
    write_git_pointer(&root, "vendor/sub/.git", "../../.git/modules/sub");
    write(&root, "vendor/sub/.gitignore", "sub_only.ts\n");
    for name in [
        "x.gen.ts",
        "parent_only.ts",
        "sub_only.ts",
        "sub_exclude.ts",
        "keep.ts",
    ] {
        write(
            &root,
            &format!("vendor/sub/{name}"),
            "export const v = 1;\n",
        );
    }

    // (c) a nested .jj: a boundary for .gitignore, with no .git beside it.
    write(&root, "jj/.jj/repo/placeholder", "");
    write(&root, "jj/.gitignore", "jj_only.ts\n");
    for name in [
        "x.gen.ts",
        "parent_only.ts",
        "jj_only.ts",
        "everywhere.ts",
        "keep.ts",
    ] {
        write(&root, &format!("jj/{name}"), "export const v = 1;\n");
    }

    // (d) a .git pointer that does not resolve.
    write_git_pointer(&root, "broken/.git", "/nonexistent/devmap/gitdir");
    write(&root, "broken/.gitignore", "broken_only.ts\n");
    for name in ["x.gen.ts", "parent_only.ts", "broken_only.ts", "keep.ts"] {
        write(&root, &format!("broken/{name}"), "export const v = 1;\n");
    }

    let excluded = watcher_equals_walk(&root, "nested repositories");
    for expected in [
        "top.gen.ts",
        "parent_only.ts",
        "root_exclude.ts",
        "everywhere.ts",
        "clone/everywhere.ts",
        "clone/clone_only.ts",
        "clone/sub/clone_only.ts",
        "vendor/sub/sub_only.ts",
        "jj/jj_only.ts",
        "jj/everywhere.ts",
        "broken/broken_only.ts",
    ] {
        assert!(
            excluded.contains(expected),
            "{expected} is excluded: {excluded:?}"
        );
    }
    for admitted in [
        "keep.ts",
        "clone/x.gen.ts",
        "clone/parent_only.ts",
        "clone/root_exclude.ts",
        "vendor/sub/x.gen.ts",
        "vendor/sub/parent_only.ts",
        "jj/x.gen.ts",
        "jj/parent_only.ts",
        "broken/x.gen.ts",
        "broken/parent_only.ts",
    ] {
        assert!(
            !excluded.contains(admitted),
            "{admitted} is admitted: {excluded:?}"
        );
    }
}

/// A `.git` pointer that does not resolve, at the walk's own root, must not
/// fail every verdict: the walk reads no exclude file from it and walks on.
#[test]
fn an_unresolvable_git_pointer_at_the_root_does_not_fail_the_verdict() {
    let root = scratch("broken-root-pointer", false);
    write_git_pointer(&root, ".git", "/nonexistent/devmap/gitdir");
    write(&root, ".gitignore", "dropped.ts\n");
    write(&root, "dropped.ts", "export const v = 1;\n");
    write(&root, "kept.ts", "export const v = 1;\n");
    let excluded = watcher_equals_walk(&root, "broken pointer at the root");
    assert_eq!(excluded, BTreeSet::from(["dropped.ts".to_string()]));
}

/// Every rule file the walk reads is one the watcher stamps, so an edit to it
/// invalidates the verdicts it backed — `.ignore` included.
#[test]
fn the_watcher_stamps_every_rule_file_the_walk_reads() {
    let root = scratch("stamps", true);
    write(&root, "src/x.ts", "export const x = 1;\n");
    let files = ignore_rule_files(&root, &root.join("src/x.ts"), false).unwrap();
    for expected in [
        root.join(".ignore"),
        root.join("src/.ignore"),
        root.join(".gitignore"),
        root.join("src/.gitignore"),
        root.join(".git/info/exclude"),
    ] {
        assert!(
            files.contains(&expected),
            "{expected:?} is a rule file the walk reads and the watcher does not stamp: {files:?}"
        );
    }
    assert!(devmap_extract::is_ignore_rule_file(
        &root.join("src/.ignore")
    ));
    assert!(devmap_extract::is_ignore_rule_file(
        &root.join(".gitignore")
    ));
    assert!(!devmap_extract::is_ignore_rule_file(&root.join("src/x.ts")));
}

/// A manifest that reads and does not parse is a refusal, as one that cannot
/// be read is: either way its module mapping is absent from resolution.
#[test]
fn manifest_parse_failures_are_recorded_as_refusals() {
    let root = scratch("manifest-parse", true);
    write(&root, "svc/go.mod", "go 1.22\n");
    write(&root, "web/tsconfig.json", "{ \"compilerOptions\": ");
    write(&root, "ok/go.mod", "module example.com/ok\n");
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    let refused: Vec<(&str, String)> = manifests
        .refusals
        .iter()
        .map(|(path, reason)| (path.as_str(), reason.to_string()))
        .collect();
    assert!(
        refused
            .iter()
            .any(|(path, reason)| *path == "svc/go.mod" && reason.contains("could not be parsed")),
        "a go.mod with no module directive: {refused:?}"
    );
    assert!(
        refused
            .iter()
            .any(|(path, reason)| *path == "web/tsconfig.json"
                && reason.contains("could not be parsed")),
        "a tsconfig.json that is not JSON: {refused:?}"
    );
    assert!(
        refused.iter().all(|(path, _)| *path != "ok/go.mod"),
        "a parseable go.mod is not refused: {refused:?}"
    );
    assert!(refused
        .iter()
        .all(|(_, reason)| reason.starts_with("unreadable:")));
}

/// An unreadable `CACHEDIR.TAG` refuses its directory and the manifest pass
/// carries on, as discovery does. It used to fail the pass outright — and
/// with it every daemon drain of the tree.
#[cfg(unix)]
#[test]
fn an_unreadable_cache_marker_refuses_its_directory_without_failing_the_pass() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("marker", true);
    write(&root, "go.mod", "module example.com/app\n");
    write(
        &root,
        "cache/CACHEDIR.TAG",
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    write(&root, "cache/go.mod", "module example.com/cached\n");
    let marker = root.join("cache/CACHEDIR.TAG");
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&marker).is_ok() {
        // Permissions do not bind for this user (root): nothing to stage.
        return;
    }
    let manifests = devmap_extract::collect_project_manifests(&root)
        .expect("one unreadable marker must not fail the whole manifest pass");
    assert!(
        manifests
            .refusals
            .iter()
            .any(|(path, reason)| path == "cache" && reason.to_string().contains("CACHEDIR.TAG")),
        "the directory is refused, with the marker named: {:?}",
        manifests.refusals
    );
    assert!(
        manifests
            .go_modules
            .iter()
            .any(|module| module.prefix == "example.com/app"),
        "and the rest of the tree is still read"
    );
    let (_, discovery) = collect_sources_with_report(&root).unwrap();
    let discovered: Vec<String> = discovery
        .refusals()
        .map(|(path, reason)| format!("{path}: {reason}"))
        .collect();
    let manifested: Vec<String> = manifests
        .refusals
        .iter()
        .filter(|(path, _)| path == "cache")
        .map(|(path, reason)| format!("{path}: {reason}"))
        .collect();
    assert_eq!(
        manifested.len(),
        1,
        "one row for the directory: {manifested:?}"
    );
    assert!(
        discovered.contains(&manifested[0]),
        "worded as discovery words it, so the two passes' rows are one: \
         {manifested:?} vs {discovered:?}"
    );
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// A manifest that is not UTF-8 is a refusal, and a FIFO or directory carrying
/// a manifest's name is skipped, never a hang and never a failed pass. A FIFO named `tsconfig.json` would block a
/// plain `read` forever, so the pass runs on a thread with a deadline.
#[cfg(unix)]
#[test]
fn hostile_manifest_shapes_are_refusals_not_hangs_or_failures() {
    let root = scratch("manifest-hostile", true);
    write(&root, "ok/go.mod", "module example.com/ok\n");
    std::fs::create_dir_all(root.join("dirmod/go.mod")).unwrap();
    std::fs::create_dir_all(root.join("bytes")).unwrap();
    std::fs::write(root.join("bytes/go.mod"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    std::fs::create_dir_all(root.join("pipe")).unwrap();
    let fifo_made = std::process::Command::new("mkfifo")
        .arg(root.join("pipe/tsconfig.json"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !fifo_made {
        // Only the FIFO case cannot be staged; the directory and non-UTF-8
        // assertions below still run.
        eprintln!("skipped: the FIFO manifest case (mkfifo is unavailable here)");
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let walked = root.clone();
    std::thread::spawn(move || {
        let _ = tx.send(devmap_extract::collect_project_manifests(&walked));
    });
    let manifests = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("a FIFO manifest must not hang the manifest pass")
        .expect("hostile manifests must not fail the pass");
    let refused: Vec<String> = manifests
        .refusals
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    // A directory or FIFO carrying a manifest's name is not a manifest (the
    // walk classes it `Other`, "never a source", by design): neither a refusal
    // nor a failure, and above all not a blocking read. A manifest whose bytes
    // cannot be read as text is a named refusal.
    let not_manifests: &[&str] = if fifo_made {
        &["dirmod/go.mod", "pipe/tsconfig.json"]
    } else {
        &["dirmod/go.mod"]
    };
    for not_a_manifest in not_manifests {
        assert!(
            refused.iter().all(|path| path != *not_a_manifest),
            "{not_a_manifest} is not a manifest: {refused:?}"
        );
    }
    assert!(
        refused.iter().any(|path| path == "bytes/go.mod"),
        "a manifest that is not text is a named refusal: {refused:?}"
    );
    assert!(
        manifests
            .go_modules
            .iter()
            .any(|module| module.prefix == "example.com/ok"),
        "and the healthy manifest is still read"
    );
}
