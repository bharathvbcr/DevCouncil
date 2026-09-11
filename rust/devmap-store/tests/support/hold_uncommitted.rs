//! Hold an uncommitted SQLite transaction in a forked child, then SIGKILL it.
//!
//! A panic in-process unwinds through rusqlite's `Drop` and rolls the
//! transaction back, which tests destructors rather than WAL recovery. The
//! child must be a separate process. It is a `fork(2)` of this test rather
//! than an interpreter, so proving recovery does not need Python.
//!
//! The parent drops every store handle before forking so the child opens a
//! fresh connection on copied memory rather than inheriting a live one.
//! `fork` in a multithreaded test process is still a compromise: only the
//! forking thread continues, and SQLite's copied global state is safe only
//! because no connection remains open. The pipe wait is `poll(2)`-bounded so
//! a child that never announces cannot hang the parent, and every path
//! SIGKILLs then `waitpid`s.

#![cfg(unix)]

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::Connection;

#[derive(Clone, Copy)]
#[allow(dead_code)] // each store test binary uses only one script
pub enum Script {
    /// Insert a generation and a node, then wait to be killed.
    TornGeneration,
    /// Close every live edge and unresolved row, then wait to be killed.
    CloseRanges,
}

pub struct Holder {
    pid: i32,
}

impl Holder {
    pub fn spawn(db: &Path, script: Script, ready: &str) -> Self {
        spawn_with_timeout(db, script, ready, Duration::from_secs(20))
    }

    pub fn kill(mut self) {
        self.reap();
    }

    fn reap(&mut self) {
        if self.pid <= 0 {
            return;
        }
        let pid = self.pid;
        self.pid = 0;
        reap_pid(pid);
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.reap();
    }
}

fn spawn_with_timeout(db: &Path, script: Script, ready: &str, timeout: Duration) -> Holder {
    let (mut rx, mut tx) = std::io::pipe().expect("pipe");
    let db = db.to_path_buf();
    let ready = ready.to_string();
    let pid = unsafe { fork() };
    match pid {
        -1 => panic!("fork failed"),
        0 => {
            drop(rx);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_child(&db, script, &ready, &mut tx);
            }));
            unsafe { exit_child(1) }
        }
        pid => {
            drop(tx);
            let fd = rx.as_raw_fd();
            if let Err(error) = wait_for_ready(&mut rx, fd, &ready, timeout) {
                reap_pid(pid);
                panic!("child never announced {ready:?}: {error}");
            }
            Holder { pid }
        }
    }
}

fn reap_pid(pid: i32) {
    unsafe {
        kill(pid, 9);
        let mut status = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let waited = waitpid(pid, &mut status, WNOHANG);
            if waited == pid || waited < 0 {
                return;
            }
            if Instant::now() >= deadline {
                let _ = waitpid(pid, &mut status, 0);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
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
        let timeout_ms = i32::try_from(remaining.as_millis().min(i32::MAX as u128)).unwrap_or(i32::MAX);
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

fn run_child(db: &Path, script: Script, ready: &str, tx: &mut impl Write) {
    let conn = Connection::open(db).expect("child opens store");
    conn.busy_timeout(Duration::from_millis(5000))
        .expect("busy_timeout");
    conn.execute_batch("BEGIN IMMEDIATE")
        .expect("BEGIN IMMEDIATE");
    match script {
        Script::TornGeneration => {
            conn.execute(
                "INSERT INTO generations (created_at, head_sha, analysis_json) VALUES (1.0, 'torn', '{}')",
                [],
            )
            .expect("torn generation");
            conn.execute(
                "INSERT INTO generation_nodes (generation_id, ordinal, file_id, name, qualified_name, kind, span_start, span_end, is_exported) SELECT last_insert_rowid(), 0, 1, 'torn', 'torn', 'Function', 0, 1, 0",
                [],
            )
            .expect("torn node");
        }
        Script::CloseRanges => {
            conn.execute(
                "INSERT INTO generations (created_at, head_sha, analysis_json) VALUES (9.0, 'torn', '{}')",
                [],
            )
            .expect("torn generation");
            let gen: i64 = conn
                .query_row("SELECT max(id) FROM generations", [], |row| row.get(0))
                .expect("generation id");
            conn.execute(
                "UPDATE edge_rows SET valid_to = ?1 WHERE valid_to IS NULL",
                [gen],
            )
            .expect("close edges");
            conn.execute(
                "UPDATE unresolved_rows SET valid_to = ?1 WHERE valid_to IS NULL",
                [gen],
            )
            .expect("close unresolved");
        }
    }
    tx.write_all(ready.as_bytes()).expect("announce");
    tx.write_all(b"\n").expect("announce newline");
    tx.flush().expect("announce flush");
    loop {
        std::thread::sleep(Duration::from_secs(600));
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
const WNOHANG: i32 = 1;

#[cfg(any(target_os = "linux", target_os = "android"))]
type Nfds = usize;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type Nfds = u32;

extern "C" {
    fn fork() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn poll(fds: *mut PollFd, nfds: Nfds, timeout: i32) -> i32;
    #[link_name = "_exit"]
    fn exit_child(code: i32) -> !;
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

    #[test]
    fn dropping_the_holder_sigkills_and_reaps() {
        let (rx, tx) = std::io::pipe().expect("pipe");
        let pid = unsafe { fork() };
        match pid {
            -1 => panic!("fork failed"),
            0 => {
                drop(rx);
                drop(tx);
                loop {
                    std::thread::sleep(Duration::from_secs(600));
                }
            }
            pid => {
                drop(rx);
                drop(tx);
                drop(Holder { pid });
                let alive = unsafe { kill(pid, 0) };
                assert_ne!(alive, 0, "drop left pid {pid} running or unreaped");
            }
        }
    }
}
