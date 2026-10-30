//! Adversarial round on `SessionStdin` (lane compiler-cheap-fixes, ec4283f0).
//!
//! Every scenario re-executes this test binary as a child with `SIGPIPE` at
//! `SIG_DFL` (the harness ignores it, which hides the bug) and checks how the
//! child ended: a signal is a break, an `Err(BrokenPipe)` is the contract.
#![cfg(unix)]
#![allow(
    unsafe_code,
    reason = "the child restores SIGPIPE's default, as the CLI does"
)]

use std::io::{IoSlice, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dc_proc::{session_command, spawn_session, Session};

const SCENARIO: &str = "ATTACK_CCF_SCENARIO";

fn reaped_session(script: &str) -> Session {
    let mut command = session_command("sh");
    command.arg("-c").arg(script);
    let mut session = spawn_session(&mut command, Duration::from_secs(5)).expect("sh starts");
    let deadline = Instant::now() + Duration::from_secs(20);
    while session.try_status().is_none() {
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(5));
    }
    session
}

fn sigpipe_default() {
    // SAFETY: `signal(2)` with `SIG_DFL`, before the scenario starts threads.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// A write that is already blocked on a full pipe when the reader exits.
fn blocked_write_then_reader_exit() {
    sigpipe_default();
    // `sleep` never reads stdin; the 4 MiB write fills the 64 KiB pipe and
    // blocks, then `sh` exits and the read end closes under the blocked write.
    let mut command = session_command("sh");
    command.arg("-c").arg("sleep 1");
    let mut session = spawn_session(&mut command, Duration::from_secs(5)).expect("sh starts");
    let mut stdin = session.take_stdin().expect("piped");
    let outcome = stdin.write_all(&vec![b'x'; 4 << 20]).map_err(|e| e.kind());
    println!("outcome={outcome:?}");
}

/// Eight threads, each with its own dead session, writing at once.
fn concurrent_writers() {
    sigpipe_default();
    let mut handles = Vec::new();
    for _ in 0..8 {
        let mut session = reaped_session("exec 0<&-; exit 0");
        let mut stdin = session.take_stdin().expect("piped");
        handles.push(std::thread::spawn(move || {
            let mut kinds = Vec::new();
            for _ in 0..50 {
                kinds.push(stdin.write_all(b"frame").map_err(|e| e.kind()));
            }
            drop(session);
            kinds
        }));
    }
    let mut broken = 0;
    for handle in handles {
        for outcome in handle.join().expect("writer thread") {
            if outcome == Err(std::io::ErrorKind::BrokenPipe) {
                broken += 1;
            }
        }
    }
    println!("broken={broken}");
}

/// The other `Write` entry points a caller might use: `write!`, vectored,
/// `BufWriter` flush-on-drop.
fn other_write_entry_points() {
    sigpipe_default();
    let mut session = reaped_session("exec 0<&-; exit 0");
    let mut stdin = session.take_stdin().expect("piped");
    let fmt = write!(stdin, "Content-Length: {}\r\n\r\n", 3).map_err(|e| e.kind());
    let vectored = stdin
        .write_vectored(&[IoSlice::new(b"a"), IoSlice::new(b"b")])
        .map_err(|e| e.kind());
    let mut buffered = std::io::BufWriter::new(stdin);
    buffered.write_all(b"xyz").expect("buffered");
    let flushed = buffered.flush().map_err(|e| e.kind());
    println!("fmt={fmt:?} vectored={vectored:?} flushed={flushed:?}");
}

/// Control: the guard must not turn SIGPIPE off for the process. A plain
/// `std::process::ChildStdin` (not a session's) written after the child is gone
/// still ends the process, as `devmap export | head` relies on.
fn unrelated_pipe_still_signals() {
    sigpipe_default();
    // First use SessionStdin on a dead child, so any global side effect would
    // already have happened.
    let mut session = reaped_session("exec 0<&-; exit 0");
    let mut guarded = session.take_stdin().expect("piped");
    let _ = guarded.write_all(b"x");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("exec 0<&-; exit 0")
        .stdin(Stdio::piped())
        .spawn()
        .expect("sh");
    // `Child::wait` would close stdin first; poll instead.
    let mut plain = child.stdin.take().expect("piped");
    while child.try_wait().expect("poll").is_none() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let outcome = plain.write_all(b"x").map_err(|e| e.kind());
    println!("plain-write-survived={outcome:?}");
}

fn run(scenario: &str, test: &str) -> (std::process::ExitStatus, String) {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(SCENARIO, scenario)
        .output()
        .expect("re-execute the test binary");
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

macro_rules! scenario_test {
    ($name:ident, $body:expr, $check:expr) => {
        #[test]
        fn $name() {
            if let Ok(wanted) = std::env::var(SCENARIO) {
                if wanted == stringify!($name) {
                    $body();
                }
                return;
            }
            let (status, stdout) = run(stringify!($name), stringify!($name));
            assert_eq!(
                status.signal(),
                None,
                "the scenario died by signal {:?}: {stdout}",
                status.signal()
            );
            assert!(status.success(), "{stdout}");
            let check: fn(&str) = $check;
            check(&stdout);
        }
    };
}

scenario_test!(
    a_blocked_write_woken_by_the_reader_exiting_is_an_error,
    blocked_write_then_reader_exit,
    |out| assert!(out.contains("outcome=Err(BrokenPipe)"), "{out}")
);

scenario_test!(
    concurrent_writers_to_dead_sessions_all_get_an_error,
    concurrent_writers,
    |out| assert!(out.contains("broken=400"), "{out}")
);

scenario_test!(
    every_write_entry_point_reports_the_closed_pipe,
    other_write_entry_points,
    |out| assert!(
        out.contains("fmt=Err(BrokenPipe) vectored=Err(BrokenPipe) flushed=Err(BrokenPipe)"),
        "{out}"
    )
);

/// Control, expected to END IN SIGNAL 13 (the guard is per descriptor, not
/// process-wide). Written as the inverse of the others.
#[test]
fn the_guard_is_not_process_wide() {
    if let Ok(wanted) = std::env::var(SCENARIO) {
        if wanted == "the_guard_is_not_process_wide" {
            unrelated_pipe_still_signals();
        }
        return;
    }
    let (status, stdout) = run("the_guard_is_not_process_wide", "the_guard_is_not_process_wide");
    assert_eq!(
        status.signal(),
        Some(libc::SIGPIPE),
        "an ordinary ChildStdin write must still raise SIGPIPE: {stdout}"
    );
}
