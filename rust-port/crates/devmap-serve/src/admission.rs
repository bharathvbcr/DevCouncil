//! One ceiling on concurrent work, shared by all three transports.
//!
//! The socket transport has always had one — `MAX_CONCURRENT_CONNECTIONS`, a
//! semaphore acquired before the per-connection task is spawned — and the
//! reasoning it carries applies unchanged to the other two: an accepted
//! connection may buffer up to a megabyte before any validation runs, so a
//! flood of them converts directly into task memory nothing bounds. The HTTP
//! and stdio transports were spawning per connection and per request with no
//! ceiling at all.
//!
//! Written once here rather than three times, because the three differ only in
//! what they do when the ceiling is reached, and that difference is the part
//! worth stating explicitly at each call site:
//!
//! * the socket transport **waits** ([`Admission::admit`]) — excess connections
//!   back up into the kernel listen backlog, which is what a local client
//!   expects;
//! * the HTTP transport **sheds** ([`Admission::try_admit`]) — a `503` with
//!   `Retry-After` is something an HTTP client already knows how to act on,
//!   while a paused accept loop is indistinguishable from a dead server;
//! * the stdio transport **waits briefly, then sheds**
//!   ([`Admission::admit_within`]) — the wait is real backpressure on the
//!   reader, and the shed is a JSON-RPC error naming the bound, so a request is
//!   never silently dropped and never hangs.
//!
//! The counters exist because shedding is invisible otherwise. A server that
//! quietly refuses one request in ten looks exactly like one that is merely
//! busy, and [`Admission::shed`] is the difference.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A ceiling on concurrently served work, with the numbers to see it working.
///
/// Cloning shares the ceiling: two sessions given clones of one `Admission`
/// draw from the same pool, which is what a host running several transports in
/// one process wants.
#[derive(Clone)]
pub struct Admission {
    permits: Arc<Semaphore>,
    limit: usize,
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    shed: Arc<AtomicUsize>,
}

/// Proof that one unit of work was admitted. Releases on drop.
pub struct Admitted {
    _permit: OwnedSemaphorePermit,
    in_flight: Arc<AtomicUsize>,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Admission {
    /// A ceiling of `limit`, which is clamped to at least one: a zero-permit
    /// pool admits nothing ever, which is a server that refuses every request
    /// rather than a server with a small budget.
    pub fn new(limit: usize) -> Self {
        let limit = limit.clamp(1, Semaphore::MAX_PERMITS);
        Self {
            permits: Arc::new(Semaphore::new(limit)),
            limit,
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            shed: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Work in flight right now.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// The high-water mark since this pool was created.
    ///
    /// The number to quote: `in_flight` sampled after a burst has ended reports
    /// zero however badly the burst behaved.
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// How much work was refused for want of a permit.
    pub fn shed(&self) -> usize {
        self.shed.load(Ordering::Relaxed)
    }

    fn hold(&self, permit: OwnedSemaphorePermit) -> Admitted {
        let now = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak.fetch_max(now, Ordering::Relaxed);
        Admitted {
            _permit: permit,
            in_flight: Arc::clone(&self.in_flight),
        }
    }

    fn refuse(&self) -> Option<Admitted> {
        self.shed.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Wait for a permit for as long as it takes.
    ///
    /// `None` only when the pool has been closed, which nothing here does.
    pub async fn admit(&self) -> Option<Admitted> {
        match Arc::clone(&self.permits).acquire_owned().await {
            Ok(permit) => Some(self.hold(permit)),
            Err(_) => None,
        }
    }

    /// Take a permit if one is free, and refuse rather than wait.
    pub fn try_admit(&self) -> Option<Admitted> {
        match Arc::clone(&self.permits).try_acquire_owned() {
            Ok(permit) => Some(self.hold(permit)),
            Err(_) => self.refuse(),
        }
    }

    /// Wait up to `wait` for a permit, then refuse.
    ///
    /// The wait is the backpressure: while it is held the caller is not reading
    /// its next request, so the pressure reaches the peer through the pipe
    /// rather than through this process's heap.
    pub async fn admit_within(&self, wait: Duration) -> Option<Admitted> {
        match tokio::time::timeout(wait, Arc::clone(&self.permits).acquire_owned()).await {
            Ok(Ok(permit)) => Some(self.hold(permit)),
            Ok(Err(_)) => None,
            Err(_) => self.refuse(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_ceiling_is_a_budget_of_one_not_a_closed_door() {
        let admission = Admission::new(0);
        assert_eq!(admission.limit(), 1);
        assert!(
            admission.try_admit().is_some(),
            "a pool that admits nothing is a server that refuses everything"
        );
    }

    #[tokio::test]
    async fn the_ceiling_holds_and_the_overflow_is_counted() {
        let admission = Admission::new(2);
        let first = admission.try_admit().expect("first");
        let second = admission.try_admit().expect("second");
        assert_eq!(admission.in_flight(), 2);
        assert_eq!(admission.peak(), 2);

        assert!(
            admission.try_admit().is_none(),
            "the third must be refused, not admitted"
        );
        assert_eq!(admission.shed(), 1, "and the refusal must be counted");
        assert_eq!(
            admission.peak(),
            2,
            "a refusal is not work in flight, so it must not move the high-water mark"
        );

        drop(first);
        assert_eq!(admission.in_flight(), 1, "a permit returns on drop");
        assert!(admission.try_admit().is_some(), "and is reusable");
        drop(second);

        // The high-water mark survives the burst it describes; `in_flight`
        // sampled afterwards would report zero.
        assert_eq!(admission.peak(), 2);
    }

    #[tokio::test]
    async fn a_bounded_wait_sheds_rather_than_hanging() {
        let admission = Admission::new(1);
        let held = admission.try_admit().expect("held");
        let started = std::time::Instant::now();
        assert!(
            admission
                .admit_within(Duration::from_millis(50))
                .await
                .is_none(),
            "a full pool must refuse the waiter, not queue it forever"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(admission.shed(), 1);
        drop(held);
        assert!(
            admission
                .admit_within(Duration::from_millis(500))
                .await
                .is_some(),
            "and a freed permit ends the wait"
        );
    }
}
