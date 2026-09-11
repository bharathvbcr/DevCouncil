//! The deadline reaches what the child started, and the drain threads come home.
//!
//! `run_bounded`'s deadline failure reads "{program} exceeded {deadline:?} and
//! was killed". Killing the direct child alone does not make that sentence
//! true. A grandchild — a git hook, a credential helper, a pager, an `sh -c`
//! fan-out — inherits the pipes, outlives its parent, and holds the write ends
//! open. Two things follow, and both are tested here:
//!
//! * the descendant runs on, past a deadline the caller was told had been
//!   enforced; and
//! * the runner's two drain threads stay blocked in `read` on a process the
//!   runner never started, for as long as *that* process lives — two threads
//!   and two file descriptors per expiry, returned to nobody, in a daemon that
//!   runs `git` on a schedule.
//!
//! Unix only, because a process group is a unix idea and so are the fixtures.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use devmap_extract::subprocess::{run_bounded, Bounds, Failure};

/// One directory per call, unique across a parallel run: macOS reports
/// `SystemTime` at microsecond resolution and this harness starts its tests in
/// the same instant, so a wall-clock stamp alone collides — one test's script
/// overwritten with another's body.
fn script(body: &str) -> (PathBuf, PathBuf) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-descendants-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("child");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

/// A child that hands the inherited pipes to a sleeper, reports that sleeper's
/// pid in `$1`, and then hangs itself.
///
/// `exec` for the second sleep, so the *direct* child is a sleeper rather than
/// a shell — otherwise a shell orphan would muddy which process the deadline
/// failed to reach. The background `sleep` keeps stdout and stderr open, which
/// is what strands the drain threads.
const HANDS_A_SLEEPER_THE_PIPES: &str = "sleep 30 &\necho $! > \"$1\"\nexec sleep 30";

/// Bounds tight enough that expiry is the certain outcome on any machine: the
/// child sleeps for thirty seconds and the deadline is three tenths of one.
fn expiring_bounds() -> Bounds {
    Bounds {
        deadline: Duration::from_millis(300),
        stdout_cap: 1 << 20,
        stderr_cap: 1 << 20,
    }
}

/// Does a process with this pid still exist? Signal 0 runs the existence and
/// permission checks and delivers nothing.
///
/// Two caveats the callers below are built around. A killed child is still a
/// pid until someone reaps it, and the kill that ends this grandchild also
/// orphans it, so `init` does the reaping a moment later — every caller polls
/// rather than sampling once. And a pid that has been reaped can in principle
/// be reused before the poll notices; on a machine that allocates pids
/// upward that needs the whole space to wrap inside the grace, so the risk is
/// a false *red*, never a false green.
fn alive(pid: i32) -> bool {
    // SAFETY: `kill` with signal 0 delivers nothing; it is the documented
    // existence check and touches no memory of ours.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Poll until `pid` is gone, up to `grace`. Returns whether it went.
fn waited_for_exit(pid: i32, grace: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < grace {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !alive(pid)
}

/// Read the pid the fixture wrote, waiting for it to appear.
///
/// A panic here is the right answer: a fixture whose grandchild never started
/// would make every assertion below vacuously true, which is the one result
/// this file must never produce.
fn grandchild_pid(pidfile: &Path) -> i32 {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if let Ok(text) = std::fs::read_to_string(pidfile) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "the fixture never reported a grandchild pid at {}; the test would \
         otherwise assert nothing",
        pidfile.display()
    );
}

/// Best effort, so a red run does not leave thirty-second sleepers behind for
/// the next of the twenty loop iterations to trip over. Always called *after*
/// the verdict has been taken.
fn reap(pids: &[i32]) {
    for pid in pids {
        // SAFETY: a kill of a pid we started; failure (already gone) is fine.
        unsafe {
            libc::kill(*pid, libc::SIGKILL);
        }
    }
}

/// The deadline's promise is about the child *and its descendants*, because
/// that is what the caller cannot see and cannot clean up. A grandchild that
/// keeps running past the deadline is a process nobody is bounding: the runner
/// has returned and told the caller it killed what it started.
#[test]
fn a_grandchild_does_not_outlive_the_deadline_that_killed_its_parent() {
    let (dir, child) = script(HANDS_A_SLEEPER_THE_PIPES);
    let pidfile = dir.join("grandchild.pid");

    let started = Instant::now();
    let failure = run_bounded(Command::new(&child).arg(&pidfile), expiring_bounds())
        .expect_err("a child that sleeps for thirty seconds cannot answer in three tenths of one");
    let elapsed = started.elapsed();

    let grandchild = grandchild_pid(&pidfile);
    // Generous, because the grace covers an orphan being reparented and reaped
    // by init, not the speed of the kill. It is still an order of magnitude
    // short of the sleeper's thirty seconds, so it cannot pass by waiting.
    let went = waited_for_exit(grandchild, Duration::from_secs(3));
    reap(&[grandchild]);
    let _ = std::fs::remove_dir_all(dir);

    assert!(
        matches!(failure, Failure::Deadline { .. }),
        "expected the deadline failure, got {failure}"
    );
    // The runner must still return on time; killing more must not mean waiting
    // longer. Well short of thirty seconds, and not a claim about machine load.
    assert!(
        elapsed < Duration::from_secs(5),
        "the runner waited on the sleeper instead of returning at its deadline: {elapsed:?}"
    );
    assert!(
        went,
        "the runner reported `{failure}`, but the grandchild it started \
         (pid {grandchild}) is still running: the deadline killed one pid and \
         left the rest of the tree to run unbounded"
    );
}

/// The runner's two drain threads block in `read` on pipes a surviving
/// descendant still holds. The runner returns `Deadline` without them, so each
/// expiry hands back two threads and two descriptors that nobody is waiting
/// for and nothing will free until the descendant exits on its own.
///
/// The count is the whole test process's, which the harness also uses, so the
/// slack below absorbs the harness's own per-test threads. Six expiries strand
/// twelve; six is the threshold, so the two readings cannot be confused.
///
/// Only where a thread count is actually readable. Elsewhere the target is
/// absent rather than green — a check that could not run must not report what a
/// check that ran and passed reports, and `libtest` has no third answer.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_deadline_does_not_strand_the_drain_threads_on_a_surviving_descendant() {
    const RUNS: usize = 6;
    const SLACK: usize = 6;

    let (dir, child) = script(HANDS_A_SLEEPER_THE_PIPES);
    let before = threads_in_this_process().expect("this platform reports its own thread count");

    let mut grandchildren = Vec::new();
    for run in 0..RUNS {
        let pidfile = dir.join(format!("grandchild-{run}.pid"));
        let failure = run_bounded(Command::new(&child).arg(&pidfile), expiring_bounds())
            .expect_err("each run must expire");
        assert!(matches!(failure, Failure::Deadline { .. }), "{failure}");
        grandchildren.push(grandchild_pid(&pidfile));
    }

    // Poll rather than sample: a thread that has hit EOF still has to be
    // scheduled to finish. Pre-fix the descendants live for thirty seconds, so
    // no amount of settling inside this window can bring the count back.
    let settling = Instant::now();
    let mut after = threads_in_this_process().expect("this platform reports its own thread count");
    while after > before + SLACK && settling.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
        after = threads_in_this_process().expect("this platform reports its own thread count");
    }

    reap(&grandchildren);
    let _ = std::fs::remove_dir_all(dir);

    assert!(
        after <= before + SLACK,
        "{RUNS} expired runs left {} extra threads alive ({before} before, {after} after): \
         the drain threads are still blocked reading pipes that a descendant of the killed \
         child is holding open, and the runner has already returned",
        after.saturating_sub(before)
    );
}

/// Threads in this process, or `None` if the platform will not say.
#[cfg(target_os = "macos")]
fn threads_in_this_process() -> Option<usize> {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `proc_pidinfo` fills at most `size` bytes of a buffer we own and
    // sized from the same type; the return value is the byte count it wrote.
    let written = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            std::ptr::from_mut(&mut info).cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // A negative count is nonsense rather than zero threads; `None` makes the
    // caller panic on it instead of comparing garbage against a threshold.
    usize::try_from(info.pti_threadnum).ok()
}

#[cfg(target_os = "linux")]
fn threads_in_this_process() -> Option<usize> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Threads:")?.trim().parse().ok())
}
