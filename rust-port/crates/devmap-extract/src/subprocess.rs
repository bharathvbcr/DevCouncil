//! One bounded subprocess, for every place the kernel shells out.
//!
//! The kernel asks `git` three questions — `HEAD` for the store's freshness
//! sentinel, `HEAD` and `ls-files` for the artifact digests, `log` for churn —
//! and until this module existed each asker ran its own child process its own
//! way. `devmap-store` drained both pipes on threads and killed at five
//! seconds; `devmap-query`'s digests called `Command::output()` with no bound
//! at all, on the path every artifact write takes — the hang the store's
//! deadline exists to prevent, reintroduced one crate up; and the churn reader
//! had its own drain thread and its own cap, closing the pipe at the cap and
//! letting `SIGPIPE` end git. Correct, and a third copy of the discipline.
//!
//! Three runners is how three disciplines drift. This is the one, and the
//! properties every caller gets by going through it:
//!
//! * **A wall-clock deadline.** The child is killed and reaped when it expires,
//!   whether it is still writing, has closed its pipes and hung, or is waiting
//!   on a terminal that is not there.
//! * **An output cap that keeps the child honest.** Bytes past the cap are
//!   read and discarded rather than the pipe being closed under the child, so
//!   it finishes on its own terms and its exit status still means what it
//!   says; the caller learns that the answer is incomplete — never a truncated
//!   answer presented as whole.
//! * **Both pipes drained concurrently**, because reading them after exit
//!   deadlocks the moment either buffer fills.
//! * **No terminal, no stdin.** `stdin` is `/dev/null`; the `git` constructor
//!   below also refuses interactive prompts and optional locks.
//!
//! What it deliberately does not decide is what a failure *means*: the store
//! treats an unavailable `HEAD` as "unavailable", the digests fall back to the
//! two fingerprints, churn reports `computed: false` with the reason. Each
//! caller keeps its contract and gains the bounds.

use std::ffi::OsStr;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Wall clock allowed for `git rev-parse HEAD`, wherever the kernel asks it.
///
/// `git` can stall on pathological repositories, network mounts or hook
/// misconfigurations; unbounded, it hung every drain batch and CLI status
/// behind it. On expiry the child is killed and the caller gets an error —
/// every asker already treats an unavailable head as "unavailable", so a
/// stalled git degrades honestly instead of wedging the daemon. One constant
/// for the store's sentinel and the artifacts' digest, so the two cannot
/// disagree about how long a head is worth waiting for.
pub const GIT_HEAD_DEADLINE: Duration = Duration::from_secs(5);

/// How much of a child the caller is prepared to wait for and to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Wall clock from spawn to exit. On expiry the child is killed.
    pub deadline: Duration,
    /// Bytes of stdout kept. Anything past it is drained and dropped, and
    /// [`Captured::stdout_truncated`] says so.
    pub stdout_cap: usize,
    /// Bytes of stderr kept, the same way.
    pub stderr_cap: usize,
}

/// What a child that exited within its bounds left behind.
#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    /// The child wrote more stdout than the cap; the rest was discarded.
    pub stdout_truncated: bool,
    pub stderr: Vec<u8>,
    pub stderr_truncated: bool,
    /// Spawn to reap.
    pub elapsed: Duration,
}

impl Captured {
    /// stdout as text, lossily. Truncation is still the caller's to check.
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// stderr as trimmed text, for error messages.
    pub fn stderr_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Why no [`Captured`] came back. A non-zero exit is *not* a failure here —
/// the child ran and answered — so it arrives as a [`Captured`] with its
/// status, and each caller decides what a refusal means for it.
#[derive(Debug)]
pub enum Failure {
    /// The program could not be started at all.
    Spawn {
        program: String,
        error: std::io::Error,
    },
    /// The child was still running at the deadline and was killed.
    Deadline { program: String, deadline: Duration },
    /// The child could not be waited on.
    Wait {
        program: String,
        error: std::io::Error,
    },
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Spawn { program, error } => write!(f, "cannot spawn {program}: {error}"),
            Failure::Deadline { program, deadline } => {
                write!(f, "{program} exceeded {deadline:?} and was killed")
            }
            Failure::Wait { program, error } => write!(f, "cannot wait for {program}: {error}"),
        }
    }
}

impl std::error::Error for Failure {}

/// A `git` invocation against `root`, with the hygiene every kernel call
/// wants: `-C root` rather than `current_dir` so a missing root is git's error
/// and not a spawn failure, no terminal prompt, and no optional index locks
/// (a read must never leave `index.lock` behind in someone's checkout).
///
/// Subcommand and flags are the caller's to add.
pub fn git(root: &Path) -> Command {
    git_with_program(OsStr::new("git"), root)
}

/// [`git`] with the program named, so a test can stand a script in for it.
pub fn git_with_program(program: &OsStr, root: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .arg("-C")
        .arg(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Run `command` to completion within `bounds`.
///
/// `stdin`, `stdout` and `stderr` are set here; anything the caller configured
/// for them is replaced, because a child holding a terminal or an undrained
/// pipe is exactly what the bounds exist to rule out.
pub fn run_bounded(command: &mut Command, bounds: Bounds) -> Result<Captured, Failure> {
    let program = command.get_program().to_string_lossy().into_owned();
    let started = Instant::now();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::Spawn {
            program: program.clone(),
            error,
        })?;

    // Both pipes are taken before anything waits: a reader that starts after
    // the child has filled its buffer starts too late.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let (stdout_done, stdout_rx) = mpsc::channel();
    let (stderr_done, stderr_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = stdout_done.send(drain(stdout_pipe, bounds.stdout_cap));
    });
    thread::spawn(move || {
        let _ = stderr_done.send(drain(stderr_pipe, bounds.stderr_cap));
    });

    let deadline = started + bounds.deadline;
    let killed = |program: &str, child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
        Failure::Deadline {
            program: program.to_string(),
            deadline: bounds.deadline,
        }
    };

    // EOF on both pipes is the cheap signal that the child is finishing; the
    // reap below is what confirms it. Waiting on the readers first means the
    // common case costs no polling at all.
    let stdout = match stdout_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(drained) => drained,
        Err(_) => return Err(killed(&program, &mut child)),
    };
    let stderr = match stderr_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(drained) => drained,
        Err(_) => return Err(killed(&program, &mut child)),
    };

    // A child can close its pipes and then hang — a hook that daemonised, a
    // process waiting on something that is not coming. The reap is bounded by
    // the same deadline for that reason.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => return Err(killed(&program, &mut child)),
            Ok(None) => thread::sleep(Duration::from_millis(1)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::Wait { program, error });
            }
        }
    };

    Ok(Captured {
        status,
        stdout: stdout.bytes,
        stdout_truncated: stdout.truncated,
        stderr: stderr.bytes,
        stderr_truncated: stderr.truncated,
        elapsed: started.elapsed(),
    })
}

struct Drained {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Read a pipe to EOF, keeping at most `cap` bytes.
///
/// Reading continues past the cap on purpose. A reader that stops leaves the
/// child blocked on a full pipe, and then only the deadline ends it — which
/// turns "this answer was long" into "this answer took ten seconds", and
/// charges every caller the full deadline for the privilege of a partial
/// result.
fn drain(pipe: Option<impl Read>, cap: usize) -> Drained {
    let mut drained = Drained {
        bytes: Vec::new(),
        truncated: false,
    };
    let Some(mut pipe) = pipe else {
        return drained;
    };
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = cap.saturating_sub(drained.bytes.len());
                if read > room {
                    drained.bytes.extend_from_slice(&chunk[..room]);
                    drained.truncated = true;
                } else {
                    drained.bytes.extend_from_slice(&chunk[..read]);
                }
            }
        }
    }
    drained
}
