//! One owner for "run `dcgrep` and read its reply".
//!
//! The three integration test binaries had three byte-identical copies of
//! this, and every copy waited on `Child::wait_with_output`, which has no
//! deadline and no size limit. That made two failure modes invisible. A
//! `dcgrep` that blocked — on a lock, on a pipe, on anything — hung the whole
//! suite with no output and nothing to attribute it to, because the elapsed
//! checks that were supposed to catch it sat *after* the wait and so never
//! ran. And a child that wrote without stopping was read into memory without
//! limit.
//!
//! Bounding it in one place bounds it for all of them, and leaves one place to
//! change when the bound is wrong.

// Each test binary compiles this whole module but uses only the part it needs.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The bound every call gets unless a test asks for a tighter one.
///
/// Deliberately generous: it exists to turn a hang into an attributable
/// failure, not to police speed. These fixtures are a handful of small files
/// and answer in milliseconds, so anything approaching this is stuck, not
/// slow, and a loaded CI box is in no danger of tripping it.
pub const CALL_BOUND: Duration = Duration::from_secs(60);

/// The most one reply may be, per pipe.
///
/// A reply for these fixtures is a few hundred bytes. This is headroom of four
/// orders of magnitude, and it is here so a runaway child fails its own test
/// instead of the machine.
pub const MAX_REPLY_BYTES: u64 = 64 * 1024 * 1024;

/// Runs `dcgrep <command>` with `request` on stdin, and returns whether it
/// exited successfully together with its raw stdout.
///
/// Bounded three ways, one for each way the unbounded version could fail
/// silently: the wait has a deadline and kills whatever outruns it, each pipe
/// is drained by its own thread so a child filling one is never mistaken for a
/// child stuck on something else, and each drain is capped.
pub fn run(command: &str, request: &[u8], within: Duration) -> (bool, Vec<u8>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcgrep"))
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dcgrep");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(request)
        .expect("write request");

    let out = drain(child.stdout.take().expect("stdout"), "stdout");
    let err = drain(child.stderr.take().expect("stderr"), "stderr");

    let deadline = Instant::now() + within;
    let mut overran = false;
    let status = loop {
        match child.try_wait().expect("child status") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                child.kill().expect("kill a child that outran its bound");
                overran = true;
                break child.wait().expect("reap the killed child");
            }
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    };

    // Joined on every path, the timeout included: killing the child closes
    // both pipes, so these return instead of outliving the call. A reader left
    // detached here is a thread still holding a pipe after the test that owns
    // it has gone.
    let stdout = out.join().expect("stdout reader thread");
    let stderr = err.join().expect("stderr reader thread");
    let stdout = stdout.unwrap_or_else(|err| panic!("`dcgrep {command}` {err}"));
    let stderr = stderr.unwrap_or_else(|err| panic!("`dcgrep {command}` {err}"));
    assert!(
        !overran,
        "`dcgrep {command}` did not return within {within:?} and was killed; \
         stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    (status.success(), stdout)
}

/// Reads one pipe to EOF on its own thread, refusing to grow without limit.
///
/// Reads one byte past the cap and then checks, rather than capping the read,
/// so an overrun is *detected* — the same bound-then-verify shape the cache
/// reader uses. Truncating instead would hand back a prefix that might still
/// parse as a whole reply.
fn drain<R: Read + Send + 'static>(
    pipe: R,
    which: &'static str,
) -> JoinHandle<Result<Vec<u8>, String>> {
    let mut pipe = pipe;
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        (&mut pipe)
            .take(MAX_REPLY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| format!("{which} could not be read: {err}"))?;
        if bytes.len() as u64 > MAX_REPLY_BYTES {
            return Err(format!(
                "wrote more than {MAX_REPLY_BYTES} bytes to {which}"
            ));
        }
        Ok(bytes)
    })
}
