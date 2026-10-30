//! A write to the stdin of a session child that has already exited is an
//! error the caller handles, not a signal that ends the caller.
//!
//! One-shot `devmap` commands restore the default `SIGPIPE` disposition so that
//! `devmap export | head` ends quietly (`devmap-cli/src/main.rs`). With that
//! disposition in force, the language-server writer thread's first frame to a
//! server that had already exited killed the whole `devmap build --lsp` process
//! with signal 13, before it printed a byte: 7 of 60 runs under load, exit
//! status 141, empty output. The race is the child exiting before the write
//! (shell start-up is milliseconds, the write is microseconds, and a loaded host
//! reorders them).
//!
//! Rust's test harness leaves `SIGPIPE` ignored, so an in-process test of the
//! write cannot see the bug. This test therefore re-executes itself as a child
//! that sets `SIG_DFL` first, as the CLI does, and the parent checks how the
//! child ended. No timing is involved: the child waits until the session child
//! is reaped, so the read end of the pipe is certainly closed when it writes.
#![cfg(unix)]
#![allow(
    unsafe_code,
    reason = "the child restores SIGPIPE's default, as the CLI does"
)]

use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::time::{Duration, Instant};

use dc_proc::{session_command, spawn_session};

const SCENARIO: &str = "DC_PROC_DEAD_STDIN_SCENARIO";
const TEST: &str = "a_write_to_a_dead_session_stdin_is_an_error_not_a_signal";

fn the_scenario() {
    // SAFETY: `signal(2)` with `SIG_DFL`, once, before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let mut command = session_command("sh");
    command.arg("-c").arg("exec 0<&-; exit 101");
    let mut session = spawn_session(&mut command, Duration::from_secs(5)).expect("sh starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while session.try_status().is_none() {
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut stdin = session.take_stdin().expect("stdin was piped");
    let outcome = stdin
        .write_all(&[b'x'; 1024])
        .and_then(|()| stdin.flush())
        .map_err(|error| error.kind());
    println!("outcome={outcome:?}");
}

#[test]
fn a_write_to_a_dead_session_stdin_is_an_error_not_a_signal() {
    if std::env::var_os(SCENARIO).is_some() {
        the_scenario();
        return;
    }
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(SCENARIO, "1")
        .output()
        .expect("re-execute the test binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.signal(),
        None,
        "the write killed its own process (signal {:?}): {stdout}",
        output.status.signal()
    );
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("outcome=Err(BrokenPipe)"),
        "the write must report the closed pipe as an error: {stdout}"
    );
}
