//! Hold an uncommitted SQLite transaction in a child process, then SIGKILL it.
//!
//! A panic in-process unwinds through rusqlite's `Drop` and rolls the
//! transaction back, which tests destructors rather than WAL recovery. The
//! child must be a separate process.
//!
//! Spawned via `hold_uncommitted_child` rather than `fork(2)`: the test harness
//! is multithreaded, and forking after rayon / libtest has started deadlocks on
//! allocator locks — measured as a 20s empty-pipe timeout under
//! `cargo test --workspace`.

#![cfg(unix)]

use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
#[allow(dead_code)] // each store test binary uses only one script
pub enum Script {
    /// Insert a generation and a node, then wait to be killed.
    TornGeneration,
    /// Close every live edge and unresolved row, then wait to be killed.
    CloseRanges,
}

impl Script {
    fn arg(self) -> &'static str {
        match self {
            Script::TornGeneration => "torn_generation",
            Script::CloseRanges => "close_ranges",
        }
    }
}

pub struct Holder {
    child: std::process::Child,
}

impl Holder {
    pub fn spawn(db: &Path, script: Script, ready: &str) -> Self {
        spawn_with_timeout(db, script, ready, Duration::from_secs(20))
    }

    pub fn kill(mut self) {
        self.reap();
    }

    fn reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.reap();
    }
}

fn spawn_with_timeout(db: &Path, script: Script, ready: &str, timeout: Duration) -> Holder {
    let bin = env!("CARGO_BIN_EXE_hold_uncommitted_child");
    let mut child = Command::new(bin)
        .arg(db)
        .arg(script.arg())
        .arg(ready)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn hold_uncommitted_child: {e}"));
    let mut stdout = child.stdout.take().expect("piped stdout");
    let fd = stdout.as_raw_fd();
    if let Err(error) = wait_for_ready(&mut stdout, fd, ready, timeout) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child never announced {ready:?}: {error}");
    }
    Holder { child }
}

/// Bounded wait for the child's readiness line. A blocking `read` would ignore
/// `timeout` for as long as the write end stayed open.
fn wait_for_ready(rx: &mut impl Read, fd: i32, ready: &str, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut buf = String::new();
    loop {
        if buf.trim() == ready {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for {ready:?}, got {buf:?}"),
            ));
        }
        let mut pfd = PollFd {
            fd,
            events: POLLIN,
            revents: 0,
        };
        let timeout_ms =
            i32::try_from(remaining.as_millis().min(i32::MAX as u128)).unwrap_or(i32::MAX);
        let rc = unsafe { poll(&mut pfd, 1 as Nfds, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if rc == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for {ready:?}, got {buf:?}"),
            ));
        }
        let hung_up = pfd.revents & (POLLHUP | POLLERR) != 0;
        if hung_up && pfd.revents & POLLIN == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("child closed pipe before {ready:?}, got {buf:?}"),
            ));
        }
        let mut chunk = [0u8; 64];
        match rx.read(&mut chunk) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("child closed pipe before {ready:?}, got {buf:?}"),
                ));
            }
            Ok(n) => buf.push_str(&String::from_utf8_lossy(&chunk[..n])),
            Err(error)
                if error.kind() == io::ErrorKind::Interrupted
                    || error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 0x0001;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;

#[cfg(any(target_os = "linux", target_os = "android"))]
type Nfds = usize;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type Nfds = u32;

extern "C" {
    fn poll(fds: *mut PollFd, nfds: Nfds, timeout: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn a_silent_writer_does_not_block_the_parent_past_the_deadline() {
        let (mut rx, tx) = std::io::pipe().expect("pipe");
        let fd = rx.as_raw_fd();
        let start = Instant::now();
        let err = wait_for_ready(&mut rx, fd, "ready", Duration::from_millis(150))
            .expect_err("must time out while the write end is still held");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "blocking read ignored the deadline: {elapsed:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(100),
            "returned before the deadline: {elapsed:?}"
        );
        drop(tx);
    }
}
