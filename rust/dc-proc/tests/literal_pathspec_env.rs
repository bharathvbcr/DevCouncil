//! Attacks on `GIT_LITERAL_PATHSPECS=1` in the kernel's git environment.
#![cfg(unix)]

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use dc_proc::{git_with_program, run_bounded, Bounds};

fn setup(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=a@example.test", "-c", "user.name=a"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn fixture(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-proc-lit-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    setup(&dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    setup(&dir, &["add", "a.txt"]);
    setup(&dir, &["commit", "-qm", "base"]);
    dir
}

/// A user whose shell exports one of git's other global pathspec switches.
/// git refuses to combine any of them with GIT_LITERAL_PATHSPECS, so every
/// kernel read that carries a pathspec dies for that user.
#[test]
fn an_inherited_global_pathspec_switch_does_not_break_a_literal_read() {
    let root = fixture("inherit");
    for var in ["GIT_GLOB_PATHSPECS", "GIT_NOGLOB_PATHSPECS", "GIT_ICASE_PATHSPECS"] {
        let mut command =
            git_with_program(OsStr::new("git"), &root, ["log", "--format=%s", "--", "a.txt"])
                .unwrap();
        // Same effect as the variable being in the kernel process's own
        // environment: `Command` inherits it unless git_env removes it.
        command.env(var, "1");
        let captured = run_bounded(
            &mut command,
            Bounds {
                deadline: Duration::from_secs(20),
                stdout_cap: 1 << 16,
                stderr_cap: 1 << 16,
            },
        )
        .unwrap();
        assert!(
            captured.status.success(),
            "{var}=1 in the environment makes `git log -- a.txt` die: {}",
            captured.stderr_trimmed()
        );
        assert_eq!(captured.stdout_lossy().trim(), "base", "{var}");
    }
}

/// Characterization, not a break: a textconv driver git spawns sees the
/// variable, because git passes its environment to every child.
#[test]
fn a_textconv_child_inherits_the_literal_switch() {
    let root = fixture("child");
    let marker = root.join("..").join(format!("dc-proc-lit-env-{}.txt", std::process::id()));
    let script = root.join("..").join(format!("dc-proc-lit-tc-{}.sh", std::process::id()));
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"LIT=$GIT_LITERAL_PATHSPECS\" >> '{}'\ncat \"$1\"\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join(".gitattributes"), "*.txt diff=probe\n").unwrap();
    setup(&root, &["config", "diff.probe.textconv", &script.display().to_string()]);
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    let mut command = git_with_program(OsStr::new("git"), &root, ["diff", "HEAD"]).unwrap();
    let _ = run_bounded(
        &mut command,
        Bounds {
            deadline: Duration::from_secs(20),
            stdout_cap: 1 << 16,
            stderr_cap: 1 << 16,
        },
    )
    .unwrap();
    let seen = std::fs::read_to_string(&marker).unwrap_or_else(|_| "textconv did not run".into());
    eprintln!("CHARACTERIZE textconv child saw: {seen:?}");
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(&script);
}
