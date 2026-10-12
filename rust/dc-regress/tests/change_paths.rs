//! `changes_between` against real git: the names it reports must not depend on
//! how the user's git config spells a diff header or quotes a path.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use dc_regress::change::{changes_between_with_program, ChangeSet, ChangeStatus};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

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
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "dc-change-paths-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    git(&root, &["init", "--initial-branch=main"]);
    root
}

fn write(root: &Path, path: &str, body: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
    std::fs::write(full, body).expect("write");
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", message, "--no-gpg-sign"]);
}

fn read(root: &Path) -> ChangeSet {
    changes_between_with_program(OsStr::new("git"), root, "HEAD~1", "HEAD").expect("change set")
}

fn names(set: &ChangeSet) -> Vec<String> {
    let mut names: Vec<String> = set.files.iter().map(|f| f.path.clone()).collect();
    names.sort();
    names
}

/// One base commit, then one that modifies `src/lib.rs`, adds `src/new.rs`
/// and renames `src/old.rs` to `src/moved.rs`.
fn fixture(label: &str) -> PathBuf {
    let root = repo(label);
    write(&root, "src/lib.rs", "pub fn a() {}\n");
    write(
        &root,
        "src/old.rs",
        "pub fn old() {}\npub fn keep() {}\npub fn more() {}\n",
    );
    commit(&root, "base");
    write(&root, "src/lib.rs", "pub fn a() {}\npub fn b() {}\n");
    write(&root, "src/new.rs", "pub fn n() {}\n");
    git(&root, &["mv", "src/old.rs", "src/moved.rs"]);
    commit(&root, "change");
    root
}

fn assert_default(set: &ChangeSet) {
    assert_eq!(names(set), ["src/lib.rs", "src/moved.rs", "src/new.rs"]);
    let moved = set
        .files
        .iter()
        .find(|f| f.path == "src/moved.rs")
        .expect("moved");
    assert_eq!(moved.renamed_from.as_deref(), Some("src/old.rs"));
    let new = set
        .files
        .iter()
        .find(|f| f.path == "src/new.rs")
        .expect("new");
    assert_eq!(new.status, ChangeStatus::Added);
}

#[test]
fn the_default_change_set_is_the_baseline() {
    assert_default(&read(&fixture("default")));
}

#[test]
fn diff_noprefix_does_not_change_the_change_set() {
    let root = fixture("noprefix");
    git(&root, &["config", "diff.noprefix", "true"]);
    assert_default(&read(&root));
}

#[test]
fn diff_srcprefix_does_not_change_the_change_set() {
    let root = fixture("src");
    git(&root, &["config", "diff.srcPrefix", "SRC/"]);
    assert_default(&read(&root));
}

#[test]
fn diff_dstprefix_does_not_change_the_change_set() {
    let root = fixture("dst");
    git(&root, &["config", "diff.dstPrefix", "DST/"]);
    assert_default(&read(&root));
}

#[test]
fn diff_mnemonicprefix_does_not_change_the_change_set() {
    let root = fixture("mnemonic");
    git(&root, &["config", "diff.mnemonicPrefix", "true"]);
    assert_default(&read(&root));
}

#[test]
fn newline_quote_and_non_ascii_names_all_appear_in_one_change_set() {
    let root = repo("odd");
    write(&root, "src/ok.py", "x = 1\n");
    commit(&root, "base");
    write(&root, "src/nl\nname.py", "a = 1\n");
    write(&root, "src/café.py", "b = 1\n");
    write(&root, "src/quo\"te.py", "c = 1\n");
    write(&root, "src/ok.py", "x = 2\n");
    commit(&root, "odd");
    let set = read(&root);
    assert_eq!(
        names(&set),
        [
            "src/café.py",
            "src/nl\nname.py",
            "src/ok.py",
            "src/quo\"te.py"
        ]
    );
    for file in &set.files {
        assert!(
            !matches!(file.status, ChangeStatus::Unreadable { .. }),
            "{}",
            file.path
        );
        assert!(!file.ranges.is_empty(), "{} lost its ranges", file.path);
    }
}

#[test]
fn a_deleted_odd_name_and_an_empty_change_are_both_answered() {
    let root = repo("del");
    write(&root, "tab\tname.py", "a = 1\n");
    write(&root, "keep.py", "k = 1\n");
    commit(&root, "base");
    std::fs::remove_file(root.join("tab\tname.py")).expect("rm");
    commit(&root, "delete");
    let set = read(&root);
    assert_eq!(names(&set), ["tab\tname.py"]);
    assert_eq!(set.files[0].status, ChangeStatus::Deleted);

    git(
        &root,
        &["commit", "--allow-empty", "-m", "empty", "--no-gpg-sign"],
    );
    assert!(read(&root).files.is_empty());
}
