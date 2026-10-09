//! The repository's .git/config does not choose what blame runs.
#![cfg(unix)]

// A repository whose .git/config points every program-bearing setting a read
// can reach at one marker script, the way a .git/config that arrived with the
// repository could. The script prints nothing and exits 0.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct Planted {
    root: PathBuf,
    marker: PathBuf,
}

impl Drop for Planted {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn planted(label: &str) -> Planted {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!(
        "planted-git-config-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let marker = base.join("ran");
    let script = base.join("planted.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&root, &["init", "--initial-branch=main"]);
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);
    // A commit carrying a signature header, so log.showSignature has
    // something to verify.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    let tree = git(&root, &["write-tree"]);
    let parent = git(&root, &["rev-parse", "HEAD"]);
    let object = format!(
        "tree {tree}\nparent {parent}\nauthor Ada <ada@example.com> {now} +0000\n\
         committer Ada <ada@example.com> {now} +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQEz\n -----END PGP SIGNATURE-----\n\nsigned\n"
    );
    let object_file = base.join("commit-object");
    std::fs::write(&object_file, object).unwrap();
    let id = git(
        &root,
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            object_file.to_str().unwrap(),
        ],
    );
    git(&root, &["update-ref", "refs/heads/main", &id]);

    let script = script.to_str().unwrap();
    for (key, value) in [
        ("core.fsmonitor", script),
        ("diff.external", script),
        ("diff.planted.textconv", script),
        ("log.showSignature", "true"),
        ("gpg.program", script),
        ("filter.planted.clean", script),
        ("filter.planted.required", "true"),
    ] {
        git(&root, &["config", key, value]);
    }
    std::fs::write(
        root.join(".git/info/attributes"),
        "* diff=planted filter=planted\n",
    )
    .unwrap();
    Planted { root, marker }
}

fn assert_nothing_ran(p: &Planted) {
    if let Ok(ran) = std::fs::read_to_string(&p.marker) {
        panic!("a program named in .git/config ran during a read:\n{ran}");
    }
}

use dc_regress::blame::blame_file_with_program;
use std::ffi::OsStr;

#[test]
fn blame_does_not_run_the_repository_s_textconv() {
    let p = planted("blame");
    let blame = blame_file_with_program(OsStr::new("git"), &p.root, "HEAD", "src/lib.rs");
    assert_nothing_ran(&p);
    let blame = blame.unwrap_or_else(|refusal| panic!("blame refused: {refusal:?}"));
    assert_eq!(blame.lines.len(), 2, "{:?}", blame.lines);
}

// A revision is caller input. Each entry point that takes one must refuse a
// revision git would read as an option, before any read runs: `--ext-diff`
// switched `diff.external` back on, and `--output=<file>` truncated a file.
// An uncommitted edit gives a working-tree diff something to hand the driver.

use dc_regress::change::changes_between_with_program;
use dc_regress::history::{
    commit_window_with_program, files_changed_with_program, resolve_blob_with_program,
    HistoryRefusal,
};

const PRECIOUS: &str = "not to be truncated\n";

fn hostile(p: &Planted) -> (PathBuf, Vec<String>) {
    let precious = p.root.parent().unwrap().join("precious.txt");
    std::fs::write(&precious, PRECIOUS).unwrap();
    std::fs::write(p.root.join("src/lib.rs"), "pub fn edited() {}\n").unwrap();
    let revisions = vec![
        "--ext-diff".to_string(),
        format!("--output={}", precious.display()),
    ];
    (precious, revisions)
}

fn assert_intact(p: &Planted, precious: &Path, what: &str) {
    assert_nothing_ran(p);
    assert_eq!(
        std::fs::read_to_string(precious).unwrap(),
        PRECIOUS,
        "{what} wrote over a file outside the repository"
    );
}

fn assert_revision_refused(refusal: &HistoryRefusal, what: &str) {
    assert!(
        matches!(refusal, HistoryRefusal::RevisionRefused { .. }),
        "{what}: expected a revision refusal, got {refusal:?}"
    );
}

#[test]
fn changes_between_refuses_a_revision_that_is_an_option() {
    let p = planted("changes-between");
    let (precious, revisions) = hostile(&p);
    for rev in &revisions {
        for (since, until) in [(rev.as_str(), "HEAD"), ("HEAD~1", rev.as_str())] {
            let what = format!("changes_between({since}, {until})");
            let result = changes_between_with_program(OsStr::new("git"), &p.root, since, until);
            assert_intact(&p, &precious, &what);
            let refusal = result.expect_err(&what);
            assert!(
                refusal.describe().contains("begins with '-'"),
                "{what}: {refusal:?}"
            );
        }
    }
    let change = changes_between_with_program(OsStr::new("git"), &p.root, "HEAD~1", "HEAD")
        .expect("a relative revision still resolves");
    assert_nothing_ran(&p);
    assert_eq!(change.files.len(), 1, "{change:?}");
}

#[test]
fn commit_window_refuses_a_revision_that_is_an_option() {
    let p = planted("commit-window");
    let (precious, revisions) = hostile(&p);
    for rev in &revisions {
        for (since, until) in [(rev.as_str(), "HEAD"), ("HEAD~1", rev.as_str())] {
            let what = format!("commit_window({since}, {until})");
            let result = commit_window_with_program(OsStr::new("git"), &p.root, since, until);
            assert_intact(&p, &precious, &what);
            assert_revision_refused(&result.expect_err(&what), &what);
        }
    }
    let window = commit_window_with_program(OsStr::new("git"), &p.root, "HEAD~1", "HEAD")
        .expect("a relative revision still resolves");
    assert_nothing_ran(&p);
    assert_eq!(window.commits.len(), 1, "{window:?}");
}

#[test]
fn resolve_blob_files_changed_and_blame_refuse_a_revision_that_is_an_option() {
    let p = planted("resolve-blob");
    let (precious, revisions) = hostile(&p);
    for rev in &revisions {
        let what = format!("resolve_blob({rev})");
        let result = resolve_blob_with_program(OsStr::new("git"), &p.root, rev, "src/lib.rs");
        assert_intact(&p, &precious, &what);
        assert_revision_refused(&result.expect_err(&what), &what);

        let what = format!("files_changed({rev})");
        let result = files_changed_with_program(OsStr::new("git"), &p.root, rev);
        assert_intact(&p, &precious, &what);
        assert_revision_refused(&result.expect_err(&what), &what);

        let what = format!("blame_file({rev})");
        let result = blame_file_with_program(OsStr::new("git"), &p.root, rev, "src/lib.rs");
        assert_intact(&p, &precious, &what);
        assert!(
            matches!(
                result,
                Err(dc_regress::blame::BlameRefusal::RevisionRefused { .. })
            ),
            "{what}: {result:?}"
        );
    }
    let blob = resolve_blob_with_program(OsStr::new("git"), &p.root, "HEAD", "src/lib.rs")
        .expect("HEAD still resolves");
    assert_nothing_ran(&p);
    assert_eq!(blob.content, "pub fn a() {}\npub fn b() {}\n");
}

/// A name git does not resolve to a commit is refused by name, not passed on.
#[test]
fn a_revision_that_names_no_commit_is_refused() {
    let p = planted("no-commit");
    let result = commit_window_with_program(OsStr::new("git"), &p.root, "no-such-ref", "HEAD");
    assert_nothing_ran(&p);
    assert_revision_refused(&result.expect_err("no-such-ref"), "no-such-ref");
}
