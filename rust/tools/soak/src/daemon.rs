//! The long-lived `devmap serve` a daemon-mode soak drives, and the bounded
//! shutdown every exit path takes it through.
//!
//! The daemon runs under `/usr/bin/time`, because its peak RSS is only
//! readable when it exits. The wrapper is spawned leading a process group of
//! its own, so every descendant — the daemon, and anything the daemon starts —
//! is in a group this soak owns and can signal as one. The group exists from
//! `exec`: there is no window in which the wrapper runs outside it, which is
//! the race the shell harness had to close with a `setsid` launcher.
//!
//! Every exit from a daemon-mode soak must take the daemon with it. Killing
//! the wrapper does not: it leaves the daemon serving, watching the workdir
//! and writing generations into the very store the *next* soak is about to
//! measure. One such orphan ran for five minutes beside a build-mode soak on
//! the same corpus, and every number that soak produced described two writers
//! rather than one.

use std::fs::File;
use std::io::{self, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use dc_proc::Signal;

/// How long a forced termination is given to be confirmed.
const KILL_CONFIRMATION: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);

/// The signals the soak sends. Production goes through dc-proc; tests inject
/// an operating system that refuses one.
pub trait Signals: Send {
    fn process(&self, pid: u32, signal: Signal) -> io::Result<()>;
    fn group(&self, leader: u32, signal: Signal) -> io::Result<()>;
}

/// The real operating system.
pub struct Os;

impl Signals for Os {
    fn process(&self, pid: u32, signal: Signal) -> io::Result<()> {
        dc_proc::signal_process(pid, signal)
    }
    fn group(&self, leader: u32, signal: Signal) -> io::Result<()> {
        dc_proc::signal_group(leader, signal)
    }
}

/// What one look for the daemon under its wrapper found.
#[derive(Debug, PartialEq, Eq)]
pub enum Identity {
    Found(u32),
    NotYet,
    /// More than one child, or an answer that is not a pid.
    Ambiguous,
    /// `pgrep` itself could not answer.
    Unexaminable,
}

/// A daemon under `/usr/bin/time`, in a process group the soak leads.
pub struct Daemon {
    wrapper: Child,
    serve_pid: Option<u32>,
    exit: Option<ExitStatus>,
    stopped: Option<bool>,
    shutdown: Duration,
    signals: Box<dyn Signals>,
}

impl Daemon {
    /// Start `program args…` under `/usr/bin/time <time_flag>`, leading its own
    /// process group, stdout discarded and the timer's report in `log`.
    pub fn spawn(
        time_flag: &str,
        program: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        cwd: &Path,
        log: &Path,
        shutdown: Duration,
    ) -> io::Result<Self> {
        let wrapper = Command::new(crate::rss::TIME)
            .arg(time_flag)
            .arg(program)
            .args(args)
            .envs(envs.iter().copied())
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(log)?)
            .process_group(0)
            .spawn()?;
        Ok(Self::adopt(wrapper, shutdown, Box::new(Os)))
    }

    /// Take over an already-spawned wrapper that leads its own process group.
    pub fn adopt(wrapper: Child, shutdown: Duration, signals: Box<dyn Signals>) -> Self {
        Self {
            wrapper,
            serve_pid: None,
            exit: None,
            stopped: None,
            shutdown,
            signals,
        }
    }

    /// The wrapper's pid, which is also the process group's id.
    pub fn leader(&self) -> u32 {
        self.wrapper.id()
    }

    /// Look once for the daemon: the wrapper's only child. Identity comes from
    /// the child of this soak's own wrapper; matching the newest `devmap` can
    /// instead sample an unrelated daemon.
    pub fn identify(&mut self) -> Identity {
        if let Some(pid) = self.serve_pid {
            return Identity::Found(pid);
        }
        let Ok(out) = Command::new("pgrep")
            .args(["-P", &self.leader().to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return Identity::Unexaminable;
        };
        match out.status.code() {
            Some(0) => {
                let text = String::from_utf8_lossy(&out.stdout);
                let text = text.trim();
                match text.parse::<u32>() {
                    Ok(pid) if text.bytes().all(|b| b.is_ascii_digit()) => {
                        self.serve_pid = Some(pid);
                        Identity::Found(pid)
                    }
                    _ => Identity::Ambiguous,
                }
            }
            Some(1) => Identity::NotYet,
            _ => Identity::Unexaminable,
        }
    }

    /// Whether the wrapper is still running. Reaps it, and keeps its status,
    /// once it has exited. An error examining it counts as running, so
    /// cleanup can never report a success it did not observe.
    pub fn wrapper_running(&mut self) -> bool {
        match self.wrapper.try_wait() {
            Ok(Some(status)) => {
                self.exit = Some(status);
                false
            }
            Ok(None) | Err(_) => true,
        }
    }

    /// Whether any member of the owned group is still alive — including a
    /// daemon its exited wrapper left reparented.
    ///
    /// The probe is definitive only when it says the group is gone. A group
    /// holding nothing but unreaped zombies answers EPERM on macOS and success
    /// on Linux, so any other answer is checked by listing the members and
    /// discarding zombies. A listing that cannot be made counts as running:
    /// cleanup must never report a stop it did not observe.
    fn group_running(&self) -> bool {
        match self.signals.group(self.leader(), Signal::Probe) {
            Err(e) if dc_proc::target_is_gone(&e) => false,
            _ => live_members(self.leader()).unwrap_or(true),
        }
    }

    fn await_exit(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        // The wrapper first: reaping it is what lets the group probe stop
        // seeing it.
        while self.wrapper_running() || self.group_running() {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
        true
    }

    /// Stop the daemon: TERM to the daemon itself, so the wrapper survives to
    /// write its peak-RSS report; escalate to KILL on the whole group after
    /// the shutdown bound. Idempotent — a second call returns the first
    /// verdict. Returns false on any shutdown that was not graceful and
    /// confirmed.
    pub fn stop(&mut self, out: &mut dyn Write) -> bool {
        if let Some(ok) = self.stopped {
            return ok;
        }
        let mut ok = true;
        match self.serve_pid {
            Some(pid) => {
                let _ = self.signals.process(pid, Signal::Terminate);
            }
            None => {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: daemon shutdown began before its child identity was verified"
                );
                ok = false;
                let _ = self.signals.group(self.leader(), Signal::Terminate);
            }
        }
        if !self.await_exit(self.shutdown) {
            let _ = writeln!(
                out,
                "SOAK FAIL: daemon shutdown exceeded {} seconds; forcing termination",
                self.shutdown.as_secs()
            );
            ok = false;
            let _ = self.signals.process(self.leader(), Signal::Kill);
            let _ = self.signals.group(self.leader(), Signal::Kill);
            if !self.await_exit(KILL_CONFIRMATION) {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: daemon shutdown could not confirm termination after SIGKILL (owned process group: {})",
                    self.leader()
                );
            }
        }
        if !self.wrapper_running() {
            // The native daemon handles TERM and returns success. A panic, a
            // failure or an outside signal is not a graceful shutdown.
            if let Some(status) = self.exit.filter(|s| !s.success()) {
                let code = status
                    .code()
                    .or_else(|| status.signal().map(|s| 128 + s))
                    .unwrap_or(-1);
                let _ = writeln!(out, "SOAK FAIL: daemon shutdown exited with status {code}");
                ok = false;
            }
        }
        let _ = out.flush();
        self.stopped = Some(ok);
        ok
    }
}

impl Drop for Daemon {
    /// A soak that unwinds past its cleanup still must not leave a daemon
    /// serving the corpus.
    fn drop(&mut self) {
        if self.stopped.is_none() {
            let _ = self.signals.group(self.leader(), Signal::Kill);
            let _ = self.wrapper.wait();
        }
    }
}

/// Whether the process group `leader` leads has a member that is not a
/// zombie; `None` when that cannot be determined.
fn live_members(leader: u32) -> Option<bool> {
    let out = Command::new("pgrep")
        .args(["-g", &leader.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    match out.status.code() {
        Some(1) => return Some(false),
        Some(0) => {}
        _ => return None,
    }
    for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
        pid.parse::<u32>().ok()?;
        let state = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let state = String::from_utf8_lossy(&state.stdout);
        let state = state.trim();
        // Gone between the listing and the look is as good as a zombie.
        if !state.is_empty() && !state.starts_with('Z') {
            return Some(true);
        }
    }
    Some(false)
}

/// Wait until `ask_symbol` observes the symbol in `mode` *and* `ask_fresh`
/// observes a fresh store, within `settle`. An OK envelope does not prove an
/// edit was indexed, so both are required; an unexaminable reply ends the
/// wait at once.
pub fn await_symbol(
    name: &str,
    mode: &str,
    settle: Duration,
    mut ask_symbol: impl FnMut() -> crate::query::Observation,
    mut ask_fresh: impl FnMut() -> crate::query::Observation,
    out: &mut dyn Write,
) -> bool {
    use crate::query::Observation::{NotYet, Observed, Unexaminable};
    let deadline = Instant::now() + settle;
    while Instant::now() < deadline {
        match ask_symbol() {
            Observed => match ask_fresh() {
                Observed => return true,
                NotYet => {}
                Unexaminable => return false,
            },
            NotYet => {}
            Unexaminable => return false,
        }
        thread::sleep(Duration::from_millis(200));
    }
    let _ = writeln!(
        out,
        "SOAK FAIL: {name} was not verified {mode} and fresh within {} seconds (IPC exchanges are separately capped at 30 seconds)",
        settle.as_secs()
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Observation::{NotYet, Observed, Unexaminable};

    #[test]
    fn absent_mutations_stop_at_the_convergence_deadline() {
        let mut out = Vec::new();
        let start = Instant::now();
        let ok = await_symbol(
            "_soak_1",
            "present",
            Duration::from_secs(1),
            || NotYet,
            || Observed,
            &mut out,
        );
        assert!(!ok);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "the settle bound did not hold"
        );
        let text = String::from_utf8(out).expect("utf-8");
        assert!(
            text.contains("not verified present and fresh within 1 seconds"),
            "{text}"
        );
    }

    #[test]
    fn a_symbol_counts_only_once_the_store_is_also_fresh() {
        let mut fresh = [NotYet, NotYet, Observed].into_iter();
        let mut asked = 0;
        let ok = await_symbol(
            "_soak_1",
            "present",
            Duration::from_secs(5),
            || {
                asked += 1;
                Observed
            },
            || fresh.next().expect("asked no more than needed"),
            &mut Vec::new(),
        );
        assert!(ok);
        assert_eq!(asked, 3);
    }

    #[test]
    fn an_unexaminable_reply_ends_the_wait_without_retrying_into_a_pass() {
        let mut out = Vec::new();
        assert!(!await_symbol(
            "x",
            "absent",
            Duration::from_secs(5),
            || Unexaminable,
            || Observed,
            &mut out
        ));
        assert!(!await_symbol(
            "x",
            "absent",
            Duration::from_secs(5),
            || Observed,
            || Unexaminable,
            &mut out
        ));
        assert!(
            out.is_empty(),
            "a refusal is not a timeout and must not be reported as one"
        );
    }

    /// A group whose only member is an exited, unreaped process holds no
    /// live work. macOS answers the group probe with EPERM there and Linux
    /// with success; neither may read as "still running".
    #[test]
    fn a_group_holding_only_a_zombie_is_not_running() {
        let wrapper = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .expect("spawn");
        thread::sleep(Duration::from_millis(300));
        let daemon = Daemon::adopt(wrapper, Duration::from_secs(1), Box::new(Os));
        assert!(!daemon.group_running(), "a zombie-only group read as live");
    }
}
