//! Review of the non-macOS `SessionStdin` SIGPIPE guard (lane
//! compiler-cheap-fixes, 28edcc7c), run on Linux.
//!
//! Each scenario re-executes this test binary as a single-threaded child with
//! `SIGPIPE` at `SIG_DFL`, so the child owns its whole signal mask, and reads
//! the outcome the child prints. libtest runs the body on a spawned thread, so
//! a process-directed SIGPIPE cannot be held pending here (the main thread
//! takes it); that case is argued from the code, not tested.
#![cfg(all(unix, not(target_os = "macos")))]
#![allow(
    unsafe_code,
    reason = "the child sets SIGPIPE's disposition and mask and reads its pending set"
)]

use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dc_proc::{Session, session_command, spawn_session};

const SCENARIO: &str = "REVIEW_CCF_LINUX_SCENARIO";

fn reaped_session() -> Session {
    let mut command = session_command("sh");
    command.arg("-c").arg("exec 0<&-; exit 0");
    let mut session = spawn_session(&mut command, Duration::from_secs(5)).expect("sh starts");
    let deadline = Instant::now() + Duration::from_secs(20);
    while session.try_status().is_none() {
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(5));
    }
    session
}

fn sigpipe_set() -> libc::sigset_t {
    // SAFETY: `sigemptyset` initialises the set before `sigaddset` reads it.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGPIPE);
        set
    }
}

fn sigpipe_pending() -> bool {
    // SAFETY: the set is initialised before `sigpending` fills it.
    unsafe {
        let mut pending: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut pending);
        libc::sigpending(&mut pending) == 0 && libc::sigismember(&pending, libc::SIGPIPE) == 1
    }
}

fn sigpipe_blocked() -> bool {
    // SAFETY: a null `set` only reads the current mask into `now`.
    unsafe {
        let mut now: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut now);
        libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut now);
        libc::sigismember(&now, libc::SIGPIPE) == 1
    }
}

fn setup(block: bool) {
    // SAFETY: disposition and mask of a single-threaded child, before any
    // session (and its drain threads, which inherit the mask) exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        if block {
            let set = sigpipe_set();
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }
    }
}

/// The caller already blocks SIGPIPE and one is pending on its own thread
/// (`pthread_kill`) before the write. The write's own SIGPIPE merges into the
/// same pending bit. The guard must not take it: it was there first.
fn thread_directed_pending_before_write() {
    setup(true);
    let mut session = reaped_session();
    let mut stdin = session.take_stdin().expect("piped");
    // SAFETY: a signal to this thread, which blocks it.
    unsafe {
        libc::pthread_kill(libc::pthread_self(), libc::SIGPIPE);
    }
    let before = sigpipe_pending();
    let outcome = stdin.write(b"frame").map_err(|e| e.kind());
    println!(
        "outcome={outcome:?} pending_before={before} pending_after={} blocked_after={}",
        sigpipe_pending(),
        sigpipe_blocked()
    );
}

/// The ordinary case: SIGPIPE unblocked and at SIG_DFL. Every write is an
/// error, the process lives, the mask is put back unblocked, nothing is left.
fn ordinary_unblocked_caller() {
    setup(false);
    let mut session = reaped_session();
    let mut stdin = session.take_stdin().expect("piped");
    let mut broken = 0;
    for _ in 0..200 {
        if stdin.write(b"frame").map_err(|e| e.kind()) == Err(std::io::ErrorKind::BrokenPipe) {
            broken += 1;
        }
    }
    let flushed = stdin.flush().map_err(|e| e.kind());
    println!(
        "broken={broken} flushed={flushed:?} pending_after={} blocked_after={}",
        sigpipe_pending(),
        sigpipe_blocked()
    );
}

/// One write larger than the pipe, to a child that never reads and exits a
/// moment later. The write blocks with part of the buffer accepted; when the
/// reader goes, Linux's `pipe_write` raises SIGPIPE and returns the short count
/// as success. The caller blocks SIGPIPE itself, so the child survives to say
/// whether the guard took the signal its write raised.
fn short_write_when_the_reader_exits() {
    setup(true);
    let mut command = session_command("sh");
    command.arg("-c").arg("sleep 1");
    let mut session = spawn_session(&mut command, Duration::from_secs(5)).expect("sh starts");
    let mut stdin = session.take_stdin().expect("piped");
    let big = vec![b'x'; 4 << 20];
    let outcome = stdin.write(&big).map_err(|e| e.kind());
    let short = matches!(outcome, Ok(n) if n < big.len());
    println!(
        "short_ok={short} outcome={outcome:?} pending_after={} blocked_after={}",
        sigpipe_pending(),
        sigpipe_blocked()
    );
}

/// Dispatches the scenario named in the environment; a no-op in the parent.
#[test]
fn review_ccf_linux_scenario_child() {
    match std::env::var(SCENARIO).as_deref() {
        Ok("thread_directed_pending_before_write") => thread_directed_pending_before_write(),
        Ok("ordinary_unblocked_caller") => ordinary_unblocked_caller(),
        Ok("short_write_when_the_reader_exits") => short_write_when_the_reader_exits(),
        _ => {}
    }
}

fn run(scenario: &str) -> (Option<i32>, Option<i32>, String) {
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "review_ccf_linux_scenario_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SCENARIO, scenario)
        .stdin(Stdio::null())
        .output()
        .expect("child runs");
    (
        output.status.code(),
        output.status.signal(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn a_sigpipe_on_the_thread_before_the_write_is_not_taken() {
    let (code, signal, stdout) = run("thread_directed_pending_before_write");
    assert_eq!((code, signal), (Some(0), None), "{stdout}");
    assert!(
        stdout.contains(
            "outcome=Err(BrokenPipe) pending_before=true pending_after=true blocked_after=true"
        ),
        "{stdout}"
    );
}

/// The guard must take the SIGPIPE a short (successful) write raised, or an
/// unblocked caller dies when the mask is put back: the mechanism behind
/// `attack_ccf_session_stdin::a_blocked_write_woken_by_the_reader_exiting_is_an_error`
/// ending in signal 13 on Linux.
#[test]
fn a_short_write_that_raised_sigpipe_leaves_nothing_pending() {
    let (code, signal, stdout) = run("short_write_when_the_reader_exits");
    assert_eq!((code, signal), (Some(0), None), "{stdout}");
    assert!(stdout.contains("short_ok=true"), "{stdout}");
    assert!(
        stdout.contains("pending_after=false blocked_after=true"),
        "{stdout}"
    );
}

#[test]
fn an_unblocked_caller_gets_errors_and_its_mask_back() {
    let (code, signal, stdout) = run("ordinary_unblocked_caller");
    assert_eq!((code, signal), (Some(0), None), "{stdout}");
    assert!(
        stdout.contains("broken=200 flushed=Ok(()) pending_after=false blocked_after=false"),
        "{stdout}"
    );
}
