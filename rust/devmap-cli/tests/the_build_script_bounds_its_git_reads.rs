//! `build.rs` reads git for the embedded build id. A hung fsmonitor daemon made
//! `git status` block forever and froze every devmap-cli build. These tests run
//! the build script's own git reader (included by path) against a fake `git`
//! that never answers, and against one that answers normally.

// `GIT_BOUND` is the build script's constant; the tests pass their own bounds.
#[allow(dead_code)]
#[path = "../build_git.rs"]
mod build_git;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn fake_git(tag: &str, body: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "devmap-build-git-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("git");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, script)
}

fn run(script: &Path, args: &[&str], bound: Duration) -> (Option<String>, Duration) {
    let started = Instant::now();
    let out = build_git::bounded_git(&OsString::from(script), Path::new("."), args, bound);
    (out, started.elapsed())
}

#[test]
fn a_git_that_never_answers_is_unknown_within_its_bound() {
    let (dir, script) = fake_git("hang", "exec sleep 300");
    let started = Instant::now();
    let dirty = build_git::dirty_state(
        &OsString::from(&script),
        Path::new("."),
        Duration::from_millis(400),
    );
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(dirty, None, "an unreadable tree is unknown, not clean");
    assert!(
        elapsed < Duration::from_secs(5),
        "the read outlived its bound: {elapsed:?}"
    );
}

#[test]
fn a_timed_out_child_is_killed_not_left_running() {
    // The fake records its pid, then hangs; the pid must be gone afterwards.
    let (dir, script) = fake_git("reap", "echo $$ > \"$0.pid\"\nexec sleep 300");
    let (out, _) = run(&script, &["status"], Duration::from_millis(300));
    assert_eq!(out, None);
    let pid: i32 = std::fs::read_to_string(dir.join("git.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    // Signal 0 only probes; ESRCH means the child was reaped.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "the timed-out git (pid {pid}) is still running");
}

#[test]
fn reads_pass_fsmonitor_off_and_no_optional_locks() {
    let (dir, script) = fake_git("args", "echo \"$@\"");
    let (out, _) = run(&script, &["status", "--porcelain"], Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&dir);
    let out = out.expect("a git that answers is read");
    assert!(
        out.starts_with("-c core.fsmonitor=false --no-optional-locks -C "),
        "unexpected git arguments: {out}"
    );
    assert!(out.ends_with("status --porcelain"), "{out}");
}

#[test]
fn a_git_that_answers_gives_clean_and_dirty() {
    let (dir, clean) = fake_git("clean", "exit 0");
    let (dir2, dirty) = fake_git("dirty", "echo ' M src/main.rs'");
    let (dir3, failing) = fake_git("fail", "exit 128");
    let b = Duration::from_secs(5);
    let seen = (
        build_git::dirty_state(&OsString::from(&clean), Path::new("."), b),
        build_git::dirty_state(&OsString::from(&dirty), Path::new("."), b),
        build_git::dirty_state(&OsString::from(&failing), Path::new("."), b),
    );
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(d);
    }
    assert_eq!(seen, (Some(false), Some(true), None));
}

#[test]
fn output_larger_than_a_pipe_buffer_is_read_in_full() {
    let (dir, script) = fake_git("big", "yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx | head -c 1000000");
    let (out, _) = run(&script, &["status"], Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.expect("answered").len() >= 999_000);
}

#[test]
fn the_dirty_env_names_the_unknown_state() {
    assert_eq!(build_git::dirty_env(Some(false)), "0");
    assert_eq!(build_git::dirty_env(Some(true)), "1");
    assert_eq!(build_git::dirty_env(None), "unknown");
}

#[test]
fn the_build_id_names_what_was_and_was_not_read() {
    assert_eq!(build_git::build_id(Some("abc"), Some(false)), "abc");
    assert_eq!(build_git::build_id(Some("abc"), Some(true)), "abc-dirty");
    assert_eq!(build_git::build_id(Some("abc"), None), "abc-dirty-unknown");
    // Outside a git checkout (git exits 128) there is no hash: the id stays
    // "unknown", with no dirty suffix.
    assert_eq!(build_git::build_id(None, None), "unknown");
    let (dir, no_repo) = fake_git("norepo", "exit 128");
    let hash = build_git::bounded_git(
        &OsString::from(&no_repo),
        Path::new("."),
        &["rev-parse", "HEAD"],
        Duration::from_secs(5),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(build_git::build_id(hash.as_deref(), None), "unknown");
}
