//! Ctrl-C for `devmap build`: the one cancellation token, tripped by SIGINT.
//!
//! The build's serial budget retry can run for minutes, and until this watcher
//! existed nothing could stop it short of the second kill. The first SIGINT now
//! trips the same [`devmap_query::Cancel`] the extraction reads, so the build
//! returns at its next file with [`devmap_extract::BuildCancelled`]. Installing
//! a tokio signal handler removes the default die-on-SIGINT, so the watcher
//! also owns the fallback: a second SIGINT, or any SIGINT once the build has
//! passed its last cancellable point ([`InterruptWatch::settle`]), exits at
//! once with [`EXIT_SIGINT`] exactly as the default action would have.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use devmap_query::Cancel;

/// The shell's convention for a process ended by SIGINT: 128 + 2.
pub(crate) const EXIT_SIGINT: u8 = 130;

/// What one SIGINT should do, given whether the build can still be cancelled
/// and whether an earlier SIGINT already asked it to stop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OnSignal {
    /// Ask the build to stop at its next file.
    Cancel,
    /// Nothing is listening for the flag (a repeat, or past the last check):
    /// end the process now.
    ExitNow,
}

pub(crate) fn on_signal(settled: bool, already_cancelled: bool) -> OnSignal {
    if settled || already_cancelled {
        OnSignal::ExitNow
    } else {
        OnSignal::Cancel
    }
}

/// A stream of SIGINT (Ctrl-C on Windows), registered at construction.
#[cfg(unix)]
struct Interrupts(Option<tokio::signal::unix::Signal>);

#[cfg(unix)]
impl Interrupts {
    /// Must run inside a tokio runtime context. A handler that cannot be
    /// installed leaves SIGINT at its default action, which still ends the
    /// build, so the failure is not fatal.
    fn register() -> Self {
        Self(tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok())
    }

    async fn next(&mut self) -> bool {
        match &mut self.0 {
            Some(stream) => stream.recv().await.is_some(),
            None => false,
        }
    }
}

#[cfg(not(unix))]
struct Interrupts;

#[cfg(not(unix))]
impl Interrupts {
    fn register() -> Self {
        Self
    }

    async fn next(&mut self) -> bool {
        tokio::signal::ctrl_c().await.is_ok()
    }
}

/// Owns the build's cancellation token and the SIGINT task that trips it.
pub(crate) struct InterruptWatch {
    cancel: Cancel,
    settled: Arc<AtomicBool>,
}

impl InterruptWatch {
    /// Watch SIGINT for the rest of the process, when a tokio runtime is
    /// running. Without one (a plain test thread) the token is simply never
    /// tripped and SIGINT keeps its default action.
    pub(crate) fn install() -> Self {
        let cancel = Cancel::new();
        let settled = Arc::new(AtomicBool::new(false));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let (task_cancel, task_settled) = (cancel.clone(), Arc::clone(&settled));
            // Registered here, synchronously, rather than inside the task: a
            // SIGINT between `install` returning and the task's first poll
            // would otherwise hit the default action or be missed.
            let _entered = runtime.enter();
            let mut interrupts = Interrupts::register();
            runtime.spawn(async move {
                while interrupts.next().await {
                    match on_signal(
                        task_settled.load(Ordering::Relaxed),
                        task_cancel.is_cancelled(),
                    ) {
                        OnSignal::Cancel => {
                            task_cancel.cancel();
                            eprintln!(
                                "interrupt: stopping the build; press Ctrl-C again to exit now"
                            );
                        }
                        OnSignal::ExitNow => std::process::exit(i32::from(EXIT_SIGINT)),
                    }
                }
            });
        }
        Self { cancel, settled }
    }

    /// The token the extraction reads.
    pub(crate) fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// The build is past its last cancellable point: from here SIGINT exits.
    pub(crate) fn settle(&self) {
        self.settled.store(true, Ordering::Relaxed);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_first_signal_cancels_and_a_repeat_or_a_settled_build_exits() {
        assert_eq!(on_signal(false, false), OnSignal::Cancel);
        assert_eq!(on_signal(false, true), OnSignal::ExitNow);
        assert_eq!(on_signal(true, false), OnSignal::ExitNow);
    }

    /// ATTACK: build.rs reads `cancel().is_cancelled()` and only then calls
    /// `settle()`. A SIGINT landing between the two is acknowledged
    /// ("stopping the build") by the task, trips the token, and nothing reads
    /// the token again: the build persists, exits 0, and the operator's Ctrl-C
    /// is silently swallowed with no exit. Replays that window deterministically.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_sigint_between_the_last_check_and_settle_is_not_swallowed() {
        let watch = InterruptWatch::install();
        // build.rs: `if interrupt.cancel().is_cancelled() { return Err(..) }`
        assert!(!watch.cancel().is_cancelled());
        // ... SIGINT lands here ...
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watch.cancel().is_cancelled() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // build.rs: `interrupt.settle();` and proceeds to persist.
        watch.settle();
        // Either the process exited (not reached) or the build must be able to
        // learn of the interrupt. settle() returns `()` (type-checked here) and
        // no later check exists, so the reachable state is cancelled-and-settled.
        let _: () = watch.settle();
        assert!(
            !watch.cancel().is_cancelled(),
            "SIGINT was acknowledged (token tripped, 'stopping the build' printed) after the \
             last check and before settle(); the build persists and exits 0 and the process did \
             not exit"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sigint_trips_the_build_token() {
        let watch = InterruptWatch::install();
        assert!(!watch.cancel().is_cancelled());
        // SAFETY: raising a signal this process has a handler installed for.
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watch.cancel().is_cancelled() {
            assert!(Instant::now() < deadline, "SIGINT never tripped the token");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
