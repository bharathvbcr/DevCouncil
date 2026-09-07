//! `run_bounded` against children that do not cooperate.
//!
//! `a_subprocess_is_bounded_in_time_and_bytes.rs` covers the shapes the runner
//! was written for. These are the ones it was not: a degenerate bound, a child
//! that hands its pipes to a grandchild and exits, a program that is not a
//! program, both pipes flooded at once, and thirty-two callers at the same
//! instant. The runner is on the artifact-write path and inside the daemon, so
//! "it returns, bounded, and says what happened" has to hold for all of them.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use devmap_extract::subprocess::{run_bounded, Bounds, Failure};

/// One script per call, unique across a parallel run: macOS reports
/// `SystemTime` at microsecond resolution and this harness starts its tests in
/// the same instant.
fn script(body: &str) -> (PathBuf, PathBuf) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("devmap-hostile-{}-{sequence}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("child");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

fn bounds(deadline: Duration, cap: usize) -> Bounds {
    Bounds {
        deadline,
        stdout_cap: cap,
        stderr_cap: cap,
    }
}

/// A zero deadline is a bound, not a hang: the runner gives up at once and
/// kills what it started.
#[test]
fn a_deadline_of_zero_gives_up_immediately() {
    let (dir, child) = script("exec sleep 30");
    let started = Instant::now();
    let failure = run_bounded(&mut Command::new(&child), bounds(Duration::ZERO, 1 << 20))
        .expect_err("a sleeper cannot finish inside no time at all");
    assert!(
        matches!(failure, Failure::Deadline { .. }),
        "a bound that expired is a deadline: {failure}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a zero deadline must not wait on the child: {:?}",
        started.elapsed()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A cap of zero keeps nothing and says the answer is incomplete — it does not
/// report an empty answer as a whole one.
#[test]
fn a_cap_of_zero_keeps_nothing_and_says_so() {
    let (dir, child) = script("echo hello; echo trouble >&2");
    let captured = run_bounded(&mut Command::new(&child), bounds(Duration::from_secs(5), 0))
        .expect("the child ran");
    assert!(captured.status.success());
    assert!(captured.stdout.is_empty());
    assert!(
        captured.stdout_truncated,
        "nothing was kept and the child wrote; the caller must not read that as an empty answer"
    );
    assert!(captured.stderr.is_empty());
    assert!(captured.stderr_truncated);
    let _ = std::fs::remove_dir_all(dir);
}

/// A child that hands its pipes to a grandchild and exits leaves the runner
/// with no EOF to wait for. It must still return at the deadline.
///
/// This is the daemonising git hook the module's doc names. The runner's own
/// case for it is a child that closes its pipes and hangs; this is the
/// opposite — the child is gone and the pipes are not.
#[test]
fn a_grandchild_holding_the_pipes_does_not_outlive_the_deadline() {
    let (dir, child) = script("sleep 30 &\nexit 0");
    let started = Instant::now();
    let outcome = run_bounded(
        &mut Command::new(&child),
        bounds(Duration::from_secs(1), 1 << 20),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(4),
        "the runner waited on a process it never started: {elapsed:?}"
    );
    match outcome {
        Err(Failure::Deadline { .. }) => {}
        other => panic!("the pipes never reached EOF, so there is no whole answer: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A program that is a directory is a spawn failure that names itself, not a
/// panic and not a silent empty answer.
#[test]
fn a_program_that_is_a_directory_names_itself() {
    let (dir, _child) = script("true");
    let failure = run_bounded(
        &mut Command::new(&dir),
        bounds(Duration::from_secs(5), 1 << 20),
    )
    .expect_err("a directory cannot be executed");
    match &failure {
        Failure::Spawn { program, .. } => assert!(
            program.contains("devmap-hostile"),
            "the failure must name what could not be started: {program}"
        ),
        other => panic!("expected a spawn failure, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A path with an interior NUL cannot be spawned, and that is a spawn failure
/// rather than a panic.
#[test]
fn a_program_path_with_a_nul_is_a_spawn_failure() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let program = OsString::from_vec(b"/bin/ec\0ho".to_vec());
    let failure = run_bounded(
        &mut Command::new(program),
        bounds(Duration::from_secs(5), 1 << 20),
    )
    .expect_err("a NUL cannot survive the exec boundary");
    assert!(
        matches!(failure, Failure::Spawn { .. }),
        "expected a spawn failure: {failure}"
    );
}

/// Both pipes flooded at once. Draining them one after the other deadlocks the
/// moment either buffer fills, which is the reason the runner takes both before
/// it waits.
#[test]
fn both_pipes_flooded_at_once_still_terminate() {
    let flood = 8 << 20;
    let (dir, child) = script(&format!(
        "head -c {flood} /dev/zero | tr '\\0' a &\n\
         head -c {flood} /dev/zero | tr '\\0' b >&2\n\
         wait"
    ));
    let started = Instant::now();
    let captured = run_bounded(
        &mut Command::new(&child),
        bounds(Duration::from_secs(20), 1 << 20),
    )
    .expect("neither pipe may block the other");
    assert!(captured.stdout_truncated && captured.stderr_truncated);
    assert_eq!(captured.stdout.len(), 1 << 20);
    assert_eq!(captured.stderr.len(), 1 << 20);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the flood cost the deadline rather than its bytes: {:?}",
        started.elapsed()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A child that writes only to stderr and exits non-zero: the status is the
/// child's, and git's own reason survives for the caller to quote.
#[test]
fn a_stderr_only_refusal_keeps_its_status_and_its_words() {
    let (dir, child) = script("echo 'fatal: not a git repository' >&2\nexit 128");
    let captured = run_bounded(
        &mut Command::new(&child),
        bounds(Duration::from_secs(5), 1 << 20),
    )
    .expect("a refusal is an answer, not a failure to run");
    assert_eq!(captured.status.code(), Some(128));
    assert!(captured.stdout.is_empty());
    assert_eq!(captured.stderr_trimmed(), "fatal: not a git repository");
    let _ = std::fs::remove_dir_all(dir);
}

/// Thirty-two callers at once. Each spawns two reader threads and two
/// channels; the runner is called from the daemon's request path, so
/// concurrent use is the normal case rather than the exotic one.
#[test]
fn thirty_two_concurrent_runs_all_answer() {
    let (dir, child) = script("echo ok");
    let mut handles = Vec::new();
    for _ in 0..32 {
        let child = child.clone();
        handles.push(std::thread::spawn(move || {
            let captured = run_bounded(
                &mut Command::new(&child),
                bounds(Duration::from_secs(20), 1 << 16),
            )
            .expect("each caller gets its own answer");
            assert!(captured.status.success());
            assert_eq!(captured.stdout_lossy().trim(), "ok");
            assert!(!captured.stdout_truncated);
        }));
    }
    for handle in handles {
        handle.join().expect("no caller may panic or deadlock");
    }
    let _ = std::fs::remove_dir_all(dir);
}
