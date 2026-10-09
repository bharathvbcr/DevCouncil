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
//! This replaced `rust/tools/soak.sh` and its two Python harness tests, which
//! sliced shell functions out of the script by string index to test them.

pub mod corpus;
pub mod query;
pub mod rss;
pub mod samples;
pub mod store;

#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod ipc;

use std::io::Write;
use std::path::PathBuf;

/// Everything an exit from the soak must undo, whichever path it leaves by —
/// the shell harness's EXIT trap. Idempotent: the normal path and an
/// interrupt may both run it.
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
    pub fn run(&mut self, out: &mut dyn Write) -> bool {
        let mut ok = true;
        if let Some(corpus) = &self.corpus {
            if let Err(e) = corpus.restore() {
                let _ = writeln!(
                    out,
                    "SOAK FAIL: cannot restore {}: {e}",
                    corpus.target.display()
                );
                ok = false;
            }
        }
        #[cfg(unix)]
        if let Some(daemon) = &mut self.daemon {
            ok &= daemon.stop(out);
        }
        for path in [&self.endpoint, &self.timer_log].into_iter().flatten() {
            let _ = std::fs::remove_file(path);
        }
        if let Some(corpus) = &self.corpus {
            let _ = std::fs::remove_file(&corpus.orig);
        }
        ok
    }
}
