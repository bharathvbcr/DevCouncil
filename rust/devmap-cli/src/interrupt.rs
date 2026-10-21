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
//!
//! Whether a signal came before or after that last point is decided by one
//! compare-and-swap on a single phase word, so a SIGINT is always either
//! acknowledged *and* seen by the build, or answered by an immediate exit.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use devmap_query::Cancel;

/// The shell's convention for a process ended by SIGINT: 128 + 2.
pub(crate) const EXIT_SIGINT: u8 = 130;

/// Cancellation can still be observed and no SIGINT has arrived.
const RUNNING: u8 = 0;
/// A SIGINT arrived while the build could still observe it.
const INTERRUPTED: u8 = 1;
/// The build passed its last cancellable point; SIGINT now exits.
const SETTLED: u8 = 2;

/// What one SIGINT should do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OnSignal {
    /// Ask the build to stop at its next file.
    Cancel,
    /// Nothing is listening for the flag (a repeat, or past the last check):
    /// end the process now.
    ExitNow,
}

/// The phase word and the token, shared by the signal task and the build.
struct Shared {
    phase: AtomicU8,
    cancel: Cancel,
}

impl Shared {
    fn new() -> Self {
        Self {
            phase: AtomicU8::new(RUNNING),
            cancel: Cancel::new(),
        }
    }

    /// One SIGINT. Only the transition out of `RUNNING` cancels.
    fn signal(&self) -> OnSignal {
        match self
            .phase
            .compare_exchange(RUNNING, INTERRUPTED, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => {
                self.cancel.cancel();
                OnSignal::Cancel
            }
            Err(_) => OnSignal::ExitNow,
        }
    }

    /// The build is past its last cancellable point. `true` when it may go on
    /// to persist; `false` when a SIGINT got there first and the build must
    /// stop.
    fn settle(&self) -> bool {
        self.phase
            .compare_exchange(RUNNING, SETTLED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
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

/// The Windows listener is created once and kept: `tokio::signal::ctrl_c()`
/// builds a fresh listener on every call, and a Ctrl-C arriving between two
/// calls is lost.
#[cfg(windows)]
struct Interrupts(Option<tokio::signal::windows::CtrlC>);

#[cfg(windows)]
impl Interrupts {
    fn register() -> Self {
        Self(tokio::signal::windows::ctrl_c().ok())
    }

    async fn next(&mut self) -> bool {
        match &mut self.0 {
            Some(stream) => stream.recv().await.is_some(),
            None => false,
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct Interrupts;

#[cfg(not(any(unix, windows)))]
impl Interrupts {
    fn register() -> Self {
        Self
    }

    async fn next(&mut self) -> bool {
        false
    }
}

/// Owns the build's cancellation token and the SIGINT task that trips it.
pub(crate) struct InterruptWatch {
    shared: Arc<Shared>,
}

impl InterruptWatch {
    /// Watch SIGINT for the rest of the process, when a tokio runtime is
    /// running. Without one (a plain test thread) the token is simply never
    /// tripped and SIGINT keeps its default action.
    pub(crate) fn install() -> Self {
        let shared = Arc::new(Shared::new());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let task_shared = Arc::clone(&shared);
            // Registered here, synchronously, rather than inside the task: a
            // SIGINT between `install` returning and the task's first poll
            // would otherwise hit the default action or be missed.
            let _entered = runtime.enter();
            let mut interrupts = Interrupts::register();
            runtime.spawn(async move {
                while interrupts.next().await {
                    match task_shared.signal() {
                        OnSignal::Cancel => eprintln!(
                            "interrupt: stopping the build; press Ctrl-C again to exit now"
                        ),
                        OnSignal::ExitNow => std::process::exit(i32::from(EXIT_SIGINT)),
                    }
                }
            });
        }
        Self { shared }
    }

    /// The token the extraction reads.
    pub(crate) fn cancel(&self) -> &Cancel {
        &self.shared.cancel
    }

    /// The build is past its last cancellable point: from here SIGINT exits.
    ///
    /// Returns `false` when a SIGINT was already acknowledged, in which case
    /// the caller must abort (exit [`EXIT_SIGINT`]) rather than persist.
    #[must_use]
    pub(crate) fn settle(&self) -> bool {
        self.shared.settle()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Runs `body` in a child copy of this test binary. SIGINT is
    /// process-global, and the watcher exits the process on a repeat or
    /// settled signal, so a signal-raising test sharing a process with any
    /// other test could kill the harness. The parent re-executes itself with
    /// this one test selected exactly; the child, recognised by the
    /// environment variable, runs the body.
    fn isolated(name: &str, body: impl FnOnce()) {
        const MARK: &str = "DEVMAP_ISOLATED_SIGINT_TEST";
        if std::env::var(MARK).as_deref() == Ok(name) {
            body();
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--test-threads=1", "--nocapture"])
            .env(MARK, name)
            .output()
            .expect("re-exec the test binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "isolated test {name} failed ({:?})\nstdout={stdout}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        // A name that no longer matches a test runs nothing and exits 0;
        // require that exactly the one selected test ran.
        assert!(
            stdout.contains("1 passed") && stdout.contains("0 failed"),
            "isolated test {name} did not run exactly one test (renamed or moved?)\nstdout={stdout}"
        );
    }

    #[test]
    fn isolated_fails_when_its_name_matches_no_test() {
        let outcome = std::panic::catch_unwind(|| {
            isolated("interrupt::tests::no_such_test", || {});
        });
        assert!(
            outcome.is_err(),
            "a stale name must fail, not pass silently"
        );
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_first_signal_cancels_and_a_repeat_or_a_settled_build_exits() {
        let running = Shared::new();
        assert_eq!(running.signal(), OnSignal::Cancel);
        assert!(running.cancel.is_cancelled());
        assert_eq!(running.signal(), OnSignal::ExitNow);
        assert!(!running.settle(), "an acknowledged SIGINT blocks settling");

        let settled = Shared::new();
        assert!(settled.settle());
        assert_eq!(settled.signal(), OnSignal::ExitNow);
        assert!(
            !settled.cancel.is_cancelled(),
            "a settled build is not cancelled"
        );
    }

    #[test]
    fn sigint_trips_the_build_token() {
        isolated("interrupt::tests::sigint_trips_the_build_token", || {
            runtime().block_on(async {
                let watch = InterruptWatch::install();
                assert!(!watch.cancel().is_cancelled());
                // SAFETY: raising a signal this process has a handler for.
                assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !watch.cancel().is_cancelled() {
                    assert!(Instant::now() < deadline, "SIGINT never tripped the token");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
        });
    }

    /// A SIGINT after the build's last look at the token but before `settle`
    /// must not be acknowledged and then swallowed: `settle` reports it, so
    /// the build aborts instead of persisting and exiting 0.
    #[test]
    fn a_sigint_between_the_last_check_and_settle_is_not_swallowed() {
        isolated(
            "interrupt::tests::a_sigint_between_the_last_check_and_settle_is_not_swallowed",
            || {
                runtime().block_on(async {
                    let watch = InterruptWatch::install();
                    assert!(!watch.cancel().is_cancelled());
                    // SAFETY: raising a signal this process has a handler for.
                    assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !watch.cancel().is_cancelled() {
                        assert!(Instant::now() < deadline);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    assert!(
                        !watch.settle(),
                        "settle() let the build go on to persist after an acknowledged SIGINT"
                    );
                });
            },
        );
    }
}
