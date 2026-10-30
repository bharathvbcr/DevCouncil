//! One file under two names is two programs to a multiplexer that reads
//! `argv[0]` (a mise or asdf shim is exactly this shape), so a `config_scope`
//! that has listed a repository's config for one name has not listed it for
//! the other: the scope's cache is keyed by the name as well as the file.
//!
//! A program is found by name through `PATH`, which is how a link is followed to
//! the one file under two names, so this is a target of its own: it sets `PATH`
//! once, before any other thread exists, and holds a single test.
#![cfg(unix)]
#![allow(
    unsafe_code,
    reason = "the one test sets PATH before any other thread exists"
)]

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

fn real_git() -> std::path::PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("a git on PATH")
}

fn setup(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=a@example.test", "-c", "user.name=a"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_scope_lists_the_config_once_per_name_a_file_runs_under() {
    let base = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("dc-proc-config-names-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    setup(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    setup(&root, &["add", "a.txt"]);
    setup(&root, &["commit", "-qm", "base"]);

    // One file that logs its arguments and runs the real git, and two names for it.
    let log = base.join("spawns.log");
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let file = base.join("real").join("shim");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_git().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["ccf-git-one", "ccf-git-two"] {
        std::os::unix::fs::symlink(&file, bin.join(name)).unwrap();
    }
    let mut path = std::env::split_paths(&std::env::var_os("PATH").unwrap()).collect::<Vec<_>>();
    path.push(bin);
    // SAFETY: this target holds one test; no other thread reads the environment.
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(path).unwrap());
    }

    {
        let _scope = dc_proc::config_scope();
        for name in ["ccf-git-one", "ccf-git-two", "ccf-git-one", "ccf-git-two"] {
            dc_proc::git_with_program(OsStr::new(name), &root, ["diff", "HEAD"])
                .unwrap_or_else(|failure| panic!("{name}: {failure}"));
        }
    }
    let log = std::fs::read_to_string(&log).unwrap_or_default();
    let listings = log
        .lines()
        .filter(|line| line.contains(" config --show-scope"))
        .count();
    assert_eq!(listings, 2, "two names, one listing each: {log}");
    let _ = std::fs::remove_dir_all(&base);
}
