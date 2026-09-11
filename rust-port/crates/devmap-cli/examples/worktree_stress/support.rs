//! Editor-retry and IPC-probe admission for the native capacity harness.
//!
//! These are the two pieces the capacity harness has to unit-test in isolation:
//! Windows sharing-violation retries must not drop an edit, and probe
//! processes must stay bounded independently of editor concurrency.

use std::io;
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Model bounded editor retries for Windows rename/delete sharing races.
///
/// Access-denied can also be permanent. Exhaustion propagates the actual error;
/// no edit or graph assertion is skipped. Other OS/errors are never retried.
pub fn retry_file_operation<T>(
    mut operation: impl FnMut() -> io::Result<T>,
    windows: bool,
    deadline_seconds: f64,
    mut on_retry: impl FnMut(),
) -> io::Result<T> {
    if !(0.0..=5.0).contains(&deadline_seconds) || !deadline_seconds.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem retry budget must be 0..5 seconds",
        ));
    }
    let deadline_at = Instant::now() + Duration::from_secs_f64(deadline_seconds);
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let remaining = deadline_at.saturating_duration_since(Instant::now());
                if !windows || !is_windows_sharing_violation(&error) || remaining.is_zero() {
                    return Err(error);
                }
                on_retry();
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
        }
    }
}

fn is_windows_sharing_violation(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// Bound measurement processes independently of editor/daemon concurrency.
pub struct ProbeAdmission {
    max: usize,
    timeout: Duration,
    state: Mutex<AdmissionState>,
    cond: Condvar,
}

struct AdmissionState {
    active: usize,
    peak: usize,
}

impl ProbeAdmission {
    pub fn new(workers: usize, timeout_seconds: f64) -> Result<Self, &'static str> {
        if !(1..=32).contains(&workers) {
            return Err("probe workers must be 1..32");
        }
        if !(0.0..=30.0).contains(&timeout_seconds) || !timeout_seconds.is_finite() {
            return Err("probe admission timeout must be 0..30 seconds");
        }
        Ok(Self {
            max: workers,
            timeout: Duration::from_secs_f64(timeout_seconds),
            state: Mutex::new(AdmissionState { active: 0, peak: 0 }),
            cond: Condvar::new(),
        })
    }

    pub fn peak(&self) -> usize {
        self.state.lock().expect("admission lock").peak
    }

    pub fn run<T, E: From<AdmissionTimeout>, F: FnOnce() -> Result<T, E>>(
        &self,
        operation: F,
    ) -> Result<T, E> {
        let mut guard = self.state.lock().expect("admission lock");
        let deadline = Instant::now() + self.timeout;
        while guard.active >= self.max {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AdmissionTimeout.into());
            }
            let (next, timed_out) = self
                .cond
                .wait_timeout(guard, remaining)
                .expect("admission wait");
            guard = next;
            if timed_out.timed_out() && guard.active >= self.max {
                return Err(AdmissionTimeout.into());
            }
        }
        guard.active += 1;
        guard.peak = guard.peak.max(guard.active);
        drop(guard);
        let result = operation();
        let mut guard = self.state.lock().expect("admission lock");
        guard.active -= 1;
        self.cond.notify_one();
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionTimeout;

impl std::fmt::Display for AdmissionTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IPC probe admission exceeded its deadline")
    }
}

impl std::error::Error for AdmissionTimeout {}

impl From<AdmissionTimeout> for io::Error {
    fn from(value: AdmissionTimeout) -> Self {
        io::Error::new(io::ErrorKind::TimedOut, value)
    }
}

/// Windows has no AF_UNIX fallback; the native `ipc_probe` is mandatory there.
pub fn require_ipc_probe(windows: bool, probe: Option<&Path>) -> io::Result<()> {
    if windows && probe.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows requires --ipc-probe (cargo build -p devmap-cli --example ipc_probe)",
        ));
    }
    Ok(())
}

/// An empty snapshot cannot prove incremental == cold; both sides must have rows.
pub fn graph_proves_equivalence(node_count: usize, edge_count: usize) -> bool {
    node_count > 0 && edge_count > 0
}

/// Startup poll, matching the Python harness: a dead child is never "ready",
/// even if a leftover endpoint still exists for the successor to reclaim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupDecision {
    Ready,
    Retry,
    Exited,
}

pub fn classify_startup(
    child_exited: bool,
    can_attempt_ipc: bool,
    ipc_ok: Option<bool>,
) -> StartupDecision {
    if child_exited {
        return StartupDecision::Exited;
    }
    if !can_attempt_ipc {
        return StartupDecision::Retry;
    }
    if ipc_ok == Some(true) {
        return StartupDecision::Ready;
    }
    StartupDecision::Retry
}

/// A failed run cannot keep `passed: true`. Cleanup failure is the same class.
pub fn finalize_pass_flag(passed: bool, has_error: bool, cleanup_failed: bool) -> bool {
    passed && !has_error && !cleanup_failed
}

/// Simultaneous editor start; times out instead of deadlocking if a party never arrives.
pub struct StartBarrier {
    parties: usize,
    timeout: Duration,
    state: Mutex<StartState>,
    cond: Condvar,
}

struct StartState {
    arrived: usize,
    generation: u64,
    broken: bool,
}

impl StartBarrier {
    pub fn new(parties: usize, timeout_seconds: f64) -> io::Result<Self> {
        if parties == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "start barrier needs at least one party",
            ));
        }
        if !(0.0..=60.0).contains(&timeout_seconds) || !timeout_seconds.is_finite() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "start barrier timeout must be 0..60 seconds",
            ));
        }
        Ok(Self {
            parties,
            timeout: Duration::from_secs_f64(timeout_seconds),
            state: Mutex::new(StartState {
                arrived: 0,
                generation: 0,
                broken: false,
            }),
            cond: Condvar::new(),
        })
    }

    pub fn wait(&self) -> io::Result<()> {
        let mut guard = self.state.lock().expect("start barrier");
        if guard.broken {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "start barrier broken",
            ));
        }
        let gen = guard.generation;
        guard.arrived += 1;
        if guard.arrived == self.parties {
            guard.arrived = 0;
            guard.generation += 1;
            self.cond.notify_all();
            return Ok(());
        }
        let deadline = Instant::now() + self.timeout;
        while guard.generation == gen && !guard.broken {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                guard.broken = true;
                self.cond.notify_all();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "start barrier timed out",
                ));
            }
            let (next, timed_out) = self
                .cond
                .wait_timeout(guard, remaining)
                .expect("start barrier wait");
            guard = next;
            if timed_out.timed_out() && guard.generation == gen && !guard.broken {
                guard.broken = true;
                self.cond.notify_all();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "start barrier timed out",
                ));
            }
        }
        if guard.broken && guard.generation == gen {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "start barrier broken",
            ));
        }
        Ok(())
    }
}
