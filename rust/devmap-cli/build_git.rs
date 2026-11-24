//! Bounded, fsmonitor-free git reads for `build.rs`.
//!
//! This file is shared: `build.rs` declares it as a module and
//! `tests/the_build_script_bounds_its_git_reads.rs` includes it by path, so the
//! build script's behaviour under a git that never answers is tested on the code
//! the build script runs, not on a copy.
//!
//! Why it exists: this repository sets `core.fsmonitor=true`. When a worktree's
//! builtin fsmonitor daemon stops answering, `git status` blocks forever in
//! `fsmonitor_ipc__send_query`, and an unbounded read here froze every build of
//! `devmap-cli` (and the gate behind it) with no output. A build identifier is
//! never worth a hung build, so every read is told not to use fsmonitor, not to
//! take optional locks, and is killed and reaped when it overruns its bound.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long one build-time git read may run before it is killed.
pub const GIT_BOUND: Duration = Duration::from_secs(10);

/// Run `git <args>` in `repo` and return its trimmed stdout.
///
/// `None` means the answer is unknown: git could not be started, exited
/// non-zero, printed non-UTF-8, or did not finish within `bound` (the child is
/// then killed and reaped).
pub fn bounded_git(
    program: &OsStr,
    repo: &Path,
    args: &[&str],
    bound: Duration,
) -> Option<String> {
    let mut child = Command::new(program)
        .args(["-c", "core.fsmonitor=false", "--no-optional-locks", "-C"])
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Drain on a thread: a full pipe must not stall the child into looking hung.
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout.read_to_end(&mut bytes);
        let _ = sender.send(read.map(|_| bytes));
    });

    let deadline = Instant::now() + bound;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(5)));
    };
    if !status.success() {
        return None;
    }
    // The child has exited, so the pipe is closed unless a grandchild kept it;
    // the reader is still bounded by what is left of the deadline.
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(250));
    let bytes = receiver.recv_timeout(remaining).ok()?.ok()?;
    String::from_utf8(bytes).ok().map(|text| text.trim().to_string())
}

/// Whether the working tree has uncommitted changes: `Some(dirty)`, or `None`
/// when `git status` could not be read in time.
pub fn dirty_state(program: &OsStr, repo: &Path, bound: Duration) -> Option<bool> {
    bounded_git(program, repo, &["status", "--porcelain"], bound).map(|text| !text.is_empty())
}

/// The `DEVMAP_GIT_DIRTY` value: `"1"`, `"0"`, or `"unknown"` when the state
/// could not be read. Consumers must treat anything but `"0"` as possibly dirty.
pub fn dirty_env(dirty: Option<bool>) -> &'static str {
    match dirty {
        Some(true) => "1",
        Some(false) => "0",
        None => "unknown",
    }
}

/// The embedded build id. Outside a git checkout there is no hash and the id is
/// plain `unknown` (a suffix would be noise); otherwise an unreadable dirty
/// state is named `-dirty-unknown` rather than guessed clean.
pub fn build_id(hash: Option<&str>, dirty: Option<bool>) -> String {
    match (hash, dirty) {
        (None, _) => "unknown".to_string(),
        (Some(hash), Some(true)) => format!("{hash}-dirty"),
        (Some(hash), Some(false)) => hash.to_string(),
        (Some(hash), None) => format!("{hash}-dirty-unknown"),
    }
}
