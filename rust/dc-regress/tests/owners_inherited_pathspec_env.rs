//! A user's own `GIT_ICASE_PATHSPECS` must not turn owner lookup into a
//! silent gap now that the kernel always sets `GIT_LITERAL_PATHSPECS`.
//!
//! One test in its own process, so setting the variable races nothing.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use dc_regress::blast::{owners_for_paths_with_program, OWNER_COMMIT_CAP};

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "commit.gpgsign=false", "-c", "user.email=ada@example.com", "-c", "user.name=Ada"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn owner_lookup_survives_a_users_icase_pathspec_switch() {
    let root = std::env::temp_dir().join(format!("dc-regress-icase-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "--initial-branch=main"]);
    std::fs::write(root.join("a.rs"), "x\n").unwrap();
    git(&root, &["add", "a.rs"]);
    git(&root, &["commit", "-q", "-m", "a"]);

    std::env::set_var("GIT_ICASE_PATHSPECS", "1");
    let (owners, gaps) = owners_for_paths_with_program(
        std::ffi::OsStr::new("git"),
        &root,
        &["a.rs".to_string()],
        OWNER_COMMIT_CAP,
        Duration::from_secs(10),
    );
    std::env::remove_var("GIT_ICASE_PATHSPECS");
    assert!(gaps.is_empty(), "owner lookup gapped under GIT_ICASE_PATHSPECS=1: {gaps:?}");
    assert_eq!(owners.len(), 1, "owners: {owners:?}");
}
