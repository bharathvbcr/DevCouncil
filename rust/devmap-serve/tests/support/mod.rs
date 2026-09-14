//! Helpers shared by the integration tests that drive a real `Daemon`.
//!
//! There is one helper here and it exists because every suite that starts a
//! daemon had written its own answer to "is it up yet?", each one a stat- or
//! connect-poll bounded by a wall clock, and each one therefore unable to tell
//! a daemon that is starting slowly from a daemon that already stopped. Each
//! poll had its own wording — "the daemon must bind its endpoint", "the daemon
//! never bound", "daemon IPC socket did not start" — and all of them named the
//! clock, for four different states: a slow start, a `run_loop` that returned
//! an error saying exactly why it would never bind, one that panicked, and an
//! endpoint that came up and was released between two polls. A gate that says
//! the same thing whether the check ran
//! or not is the shape this repository treats as worse than a visible failure,
//! so the answer has one owner now.
//!
//! Measured, on an idle machine, with the endpoint deliberately occupied so the
//! daemon could not possibly bind: `run_loop` returned `devmap IPC endpoint is
//! already active` in milliseconds, and a 500 x 20 ms stat-poll spent **11.5 s**
//! before reporting that the daemon "did not bind". That is the same shape, and
//! within 2% the same runtime, as the 11.30 s flake this crate's binary-
//! retirement test was producing under load — which is why the fix is to stop
//! asking the filesystem and start asking the daemon, not to lengthen the wait.

use std::time::Duration;

use devmap_serve::Daemon;

/// How long `run_loop` may take to either bring its endpoint up or give up.
///
/// This is a hang bound and nothing else. The ordinary path resolves on the
/// daemon's own endpoint signal, which costs no wall clock, and a daemon that
/// fails to start resolves on its task ending — so the only way to reach this
/// is `run_loop` neither binding nor returning. Sized accordingly: generous
/// enough that no loaded machine reaches it, which a budget that fires only on
/// a deadlock can afford to be.
const STARTUP_HANG_BOUND: Duration = Duration::from_secs(60);

/// Wait until `daemon`'s IPC endpoint is live, or until the task running it
/// stops trying — and say which.
///
/// `daemon` must be a clone of the one moved into `running`; both halves share
/// one `Arc`, so the clone carries the same endpoint signal.
pub async fn serving_or_dead(
    daemon: &Daemon,
    running: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let raced = tokio::time::timeout(STARTUP_HANG_BOUND, async {
        tokio::select! {
            // Biased, endpoint first. The signal is latched, so this arm is
            // ready if the endpoint ever came up — including when the daemon
            // has since retired and both arms are ready at once. An unbiased
            // select would choose between "it started" and "it stopped" at
            // random, which is how a short-idle daemon's startup would come to
            // fail one run in two.
            biased;
            () = daemon.wait_until_serving() => {}
            ended = running => {
                panic!("the daemon stopped before binding its endpoint: {ended:?}")
            }
        }
    })
    .await;
    assert!(
        raced.is_ok(),
        "`run_loop` neither brought its endpoint up nor returned within \
         {STARTUP_HANG_BOUND:?}: it is hung, not slow"
    );
}
