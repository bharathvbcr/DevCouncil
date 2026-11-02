//! Sustained-load soak: repeated edit -> rebuild -> query cycles against one
//! store, asserting that nothing drifts over time. The gates prove a *single*
//! build is correct; this proves the hundredth is too.
//!
//! It fails on any build or query error, unbounded database growth, unbounded
//! peak RSS, or a graph digest that changes when the source has been restored.
//!
//! Two modes, because the two processes fail differently. The default drives
//! `devmap build` — a short-lived process, so its peak RSS is measurable per
//! cycle. `--daemon` drives a long-lived `devmap serve` through the same edit
//! cycles with queries over IPC, which is the process an agent host actually
//! keeps open and the only one where a leak accumulates across hours.
//!
//! Build mode also reports the Medium-repository targets (one-file incremental
//! build p95 <= 120 ms, peak RSS < 512 MB; see [`stats`]) as summary lines
//! ahead of the verdict, without failing on them: they describe the kernel,
//! not the harness. `--readers N --duration SECS` is a third mode in which one
//! writer keeps editing and rebuilding while N readers query the same store,
//! and any busy, locked or other reader error fails the run ([`readers`]).
//!
//! This replaced `rust/tools/soak.sh` and its two Python harness tests, which
//! sliced shell functions out of the script by string index to test them.

pub mod args;
pub mod corpus;
pub mod diskio;
pub mod query;
pub mod readers;
pub mod rss;
pub mod samples;
pub mod stats;
pub mod store;

#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod ipc;

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The interrupt path, in the one order that lets an interrupted soak exit
/// with the signal's code and no failure line: take the cleanup lock, then
/// kill the measured builds, then clean up. The kill wakes the main thread out
/// of its build with an error; because this thread already holds the lock, the
/// main thread cannot reach its own cleanup and exit 1 first, and the failure
/// it reports on the way goes through [`QuietAfterInterrupt`], which drops it.
/// The caller exits while still holding the returned guard.
pub fn stop_for_interrupt<'a>(
    cleanup: &'a Mutex<Cleanup>,
    builds: &rss::Builds,
    out: &mut dyn Write,
) -> MutexGuard<'a, Cleanup> {
    let mut guard = cleanup.lock().unwrap_or_else(PoisonError::into_inner);
    builds.interrupt();
    guard.run(out);
    guard
}

/// The main thread's output: passed through until the soak is interrupted,
/// dropped after. What it says once an interrupt has killed its build ("SOAK
/// FAIL: initial build") is the interrupt's echo, not a finding.
pub struct QuietAfterInterrupt<'a, W> {
    pub inner: W,
    pub builds: &'a rss::Builds,
}

impl<W: Write> Write for QuietAfterInterrupt<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.builds.is_interrupted() {
            return Ok(buf.len());
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Everything an exit from the soak must undo, whichever path it leaves by —
/// the shell harness's EXIT trap. The normal path and an interrupt may both
/// run it.
#[derive(Default)]
pub struct Cleanup {
    pub corpus: Option<corpus::Corpus>,
    #[cfg(unix)]
    pub daemon: Option<daemon::Daemon>,
    pub endpoint: Option<PathBuf>,
    /// The `/usr/bin/time` report. It is written inside the corpus, so a run
    /// that leaves it behind changes the tree the next run measures.
    pub timer_log: Option<PathBuf>,
}

impl Cleanup {
    /// Restore the corpus, stop the daemon, remove what the run created.
    /// False when any of it could not be confirmed.
    ///
    /// What succeeds is consumed, so a second run — the interrupt thread
    /// arriving after the normal path — repeats only what has not yet
    /// succeeded, and is silent when nothing is left. A restore that fails
    /// keeps the pristine copy and names it: deleting it would lose the only
    /// copy of the file the soak was editing.
    pub fn run(&mut self, out: &mut dyn Write) -> bool {
        let mut ok = true;
        if let Some(corpus) = &self.corpus {
            match corpus.restore() {
                Ok(()) => {
                    let _ = std::fs::remove_file(&corpus.orig);
                    self.corpus = None;
                }
                Err(e) => {
                    let _ = writeln!(
                        out,
                        "SOAK FAIL: cannot restore {}: {e}; the original is kept at {}",
                        corpus.target.display(),
                        corpus.orig.display()
                    );
                    ok = false;
                }
            }
        }
        #[cfg(unix)]
        if let Some(daemon) = &mut self.daemon {
            // Idempotent on its own: a second stop returns the first verdict.
            ok &= daemon.stop(out);
        }
        for path in [self.endpoint.take(), self.timer_log.take()]
            .into_iter()
            .flatten()
        {
            let _ = std::fs::remove_file(path);
        }
        ok
    }
}
