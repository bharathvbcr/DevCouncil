//! A disk-write budget for a process the soak drives, and the sampler that
//! holds it to one.
//!
//! macOS reported a `devmap` process for dirtying 549.76 GB of file-backed
//! memory over 4,281 s — 128 MB/s for 71 minutes — and five more for 2–34 GB.
//! Every gate was green: the soak measured RSS and store size, and a store
//! that is rewritten in place stays the same size while the disk underneath
//! it wears out. So the daemon is also held to a write *rate*.
//!
//! Modelled on GitPulse's test-process watchdog
//! (`src-tauri/src/test_support/env.rs`, module `diskio`): a sustained rate
//! averaged over a window, so a legitimate burst — a first build, a full
//! re-extraction after an upgrade — is not mistaken for a runaway, and a
//! budget that cannot be measured is reported as not measured, never as met.
//! The counters come from [`dc_proc::diskio`], which owns the one unsafe call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use dc_proc::diskio::WriteCounters;

/// The default sustained rate: 40 MiB/s, GitPulse's test-process budget.
pub const MAX_WRITE_RATE: u64 = 40 * 1024 * 1024;
/// How long the rate is averaged over.
pub const RATE_WINDOW: Duration = Duration::from_secs(30);
const SAMPLE_EVERY: Duration = Duration::from_millis(500);
/// Samples kept, so a sampler left running for days holds a bounded history:
/// 172,800 half-second samples is 24 hours, ~5.5 MB. Past it the oldest half
/// is thinned to every other sample, which keeps the span and halves the
/// resolution of the oldest part — a window can only be judged more coarsely,
/// never skipped.
const MAX_SAMPLES: usize = 172_800;

/// A sustained-rate budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Bytes per second, averaged over `window`.
    pub rate: u64,
    pub window: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            rate: MAX_WRITE_RATE,
            window: RATE_WINDOW,
        }
    }
}

/// One reading, at an offset from when sampling started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub at: Duration,
    pub counters: WriteCounters,
}

/// The worst sustained rate in a history: the largest average, over any span
/// at least `window` long that starts and ends on a sample, of the logical
/// writes (the larger counter, and the one the incident was reported in).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorstWindow {
    pub from: Duration,
    pub to: Duration,
    pub bytes: u64,
    pub rate: f64,
}

/// The worst window in `readings`, or `None` when the history is shorter than
/// one window — too young to have a rate worth judging, which a caller must
/// report as not measured rather than as within budget.
///
/// Every sample is a window end, and its start is the latest sample at least
/// `window` before it, so the span judged is as close to `window` as the
/// sampling allows and a burst is never diluted by averaging it over the
/// whole run.
pub fn worst_window(readings: &[Reading], window: Duration) -> Option<WorstWindow> {
    let mut worst: Option<WorstWindow> = None;
    let mut start = 0usize;
    for end in 0..readings.len() {
        // Advance the start while the next sample would still leave a span of
        // at least `window`.
        while start + 1 < end && readings[end].at - readings[start + 1].at >= window {
            start += 1;
        }
        let (first, last) = (readings[start], readings[end]);
        let span = last.at.saturating_sub(first.at);
        if span < window || span.is_zero() {
            continue;
        }
        let bytes = last
            .counters
            .logical_bytes
            .saturating_sub(first.counters.logical_bytes);
        let rate = bytes as f64 / span.as_secs_f64();
        if worst.is_none_or(|w| rate > w.rate) {
            worst = Some(WorstWindow {
                from: first.at,
                to: last.at,
                bytes,
                rate,
            });
        }
    }
    worst
}

/// What a sampled run concluded.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// The worst window stayed within the budget.
    Within(WorstWindow),
    /// The worst window broke it.
    Over(WorstWindow),
    /// No verdict: the platform has no counter, the process could not be
    /// read, or the run was shorter than one window. The reason says which.
    NotRun(String),
}

/// Judge a history against a budget. Pure, so the policy is tested without
/// writing a byte.
pub fn judge(readings: &[Reading], budget: Budget) -> Verdict {
    match worst_window(readings, budget.window) {
        None => Verdict::NotRun(format!(
            "{} sample(s) spanning {:?}, shorter than the {:?} window",
            readings.len(),
            readings
                .last()
                .zip(readings.first())
                .map_or(Duration::ZERO, |(l, f)| l.at.saturating_sub(f.at)),
            budget.window
        )),
        Some(w) if w.rate > budget.rate as f64 => Verdict::Over(w),
        Some(w) => Verdict::Within(w),
    }
}

/// Whether this platform has a counter to sample. Where it does, a sampler
/// that cannot start is a failure; where it does not, the budget is NOT RUN.
pub const PLATFORM_SAMPLES: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// What an idle daemon — no edit, no query — may write over the idle stretch.
pub const IDLE_MAX_BYTES: u64 = 1024 * 1024;

/// What an idle stretch concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleVerdict {
    Within(u64),
    Over(u64),
    /// Either reading could not be taken: never a pass.
    Unreadable,
}

/// Judge the logical bytes written between two readings taken across an idle stretch,
/// with nothing for the daemon to do.
pub fn judge_idle(before: Option<WriteCounters>, after: Option<WriteCounters>) -> IdleVerdict {
    let (Some(before), Some(after)) = (before, after) else {
        return IdleVerdict::Unreadable;
    };
    let bytes = after.logical_bytes.saturating_sub(before.logical_bytes);
    if bytes > IDLE_MAX_BYTES {
        IdleVerdict::Over(bytes)
    } else {
        IdleVerdict::Within(bytes)
    }
}

pub fn mib_per_s(rate: f64) -> f64 {
    rate / (1024.0 * 1024.0)
}

/// Halve the resolution of the oldest half when the history is full.
fn thin(readings: &mut Vec<Reading>) {
    if readings.len() < MAX_SAMPLES {
        return;
    }
    let half = readings.len() / 2;
    let mut kept: Vec<Reading> = readings[..half].iter().step_by(2).copied().collect();
    kept.extend_from_slice(&readings[half..]);
    *readings = kept;
}

/// A background thread reading one process's counters every half second.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    readings: Arc<Mutex<Vec<Reading>>>,
    /// Why sampling stopped early, when it did.
    failure: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Sampler {
    /// Start sampling `pid`. Fails — rather than returning a sampler that
    /// will report nothing — when the first reading cannot be taken.
    pub fn start(pid: u32) -> Result<Self, String> {
        Self::start_with(pid, SAMPLE_EVERY, dc_proc::diskio::write_counters)
    }

    fn start_with(
        pid: u32,
        every: Duration,
        read: fn(u32) -> std::io::Result<WriteCounters>,
    ) -> Result<Self, String> {
        let started = Instant::now();
        let first = read(pid).map_err(|error| {
            format!("cannot read the disk-write counters of process {pid}: {error}")
        })?;
        let readings = Arc::new(Mutex::new(vec![Reading {
            at: Duration::ZERO,
            counters: first,
        }]));
        let stop = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let thread = {
            let (stop, readings, failure) = (
                Arc::clone(&stop),
                Arc::clone(&readings),
                Arc::clone(&failure),
            );
            std::thread::Builder::new()
                .name("soak-diskio".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(every);
                        match read(pid) {
                            Ok(counters) => {
                                let mut guard = readings
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                guard.push(Reading {
                                    at: started.elapsed(),
                                    counters,
                                });
                                thin(&mut guard);
                            }
                            Err(error) => {
                                *failure
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    Some(format!("process {pid} stopped answering: {error}"));
                                return;
                            }
                        }
                    }
                })
                .map_err(|error| format!("cannot start the disk-write sampler: {error}"))?
        };
        Ok(Self {
            stop,
            readings,
            failure,
            thread: Some(thread),
        })
    }

    /// The history so far, without stopping.
    pub fn readings(&self) -> Vec<Reading> {
        self.readings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Stop sampling and return the history and, when sampling ended early,
    /// why. A process that exited before it was stopped is such an ending:
    /// its history is still judged, and the reason travels beside it.
    pub fn finish(mut self) -> (Vec<Reading>, Option<String>) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let failure = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        (self.readings(), failure)
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64, logical: u64) -> Reading {
        Reading {
            at: Duration::from_secs(secs),
            counters: WriteCounters {
                storage_bytes: logical / 2,
                logical_bytes: logical,
            },
        }
    }

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn a_history_shorter_than_the_window_is_not_a_pass() {
        let readings = [at(0, 0), at(10, 100 * MIB), at(29, 100 * MIB)];
        assert!(matches!(
            judge(&readings, Budget::default()),
            Verdict::NotRun(_)
        ));
        assert!(matches!(judge(&[], Budget::default()), Verdict::NotRun(_)));
    }

    #[test]
    fn a_steady_rate_under_the_budget_is_within_it() {
        let readings: Vec<Reading> = (0..=120).map(|s| at(s, s * 10 * MIB)).collect();
        match judge(&readings, Budget::default()) {
            Verdict::Within(w) => assert!((mib_per_s(w.rate) - 10.0).abs() < 0.01),
            other => panic!("{other:?}"),
        }
    }

    /// The incident's shape: quiet, then a sustained 128 MB/s. Averaged over
    /// the whole run it would hide under the budget; a sliding window does not.
    #[test]
    fn a_sustained_burst_is_not_diluted_by_a_quiet_run() {
        let mut readings: Vec<Reading> = (0..=600).map(|s| at(s, 0)).collect();
        let rate = 128_000_000;
        readings.extend((1..=60).map(|s| at(600 + s, s * rate)));
        let whole_run = (60 * rate) as f64 / 660.0;
        assert!(
            whole_run < MAX_WRITE_RATE as f64,
            "the whole-run mean hides it"
        );
        match judge(&readings, Budget::default()) {
            Verdict::Over(w) => {
                assert!(w.from >= Duration::from_secs(600), "{w:?}");
                assert!((w.rate - rate as f64).abs() / (rate as f64) < 0.01, "{w:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A first build's burst shorter than the window, averaged over the
    /// window, is within budget: the window is what tells a burst from a loop.
    #[test]
    fn a_short_burst_averaged_over_the_window_is_within_it() {
        let mut readings = vec![at(0, 0), at(5, 150 * MIB)];
        readings.extend((6..=60).map(|s| at(s, 150 * MIB)));
        assert!(matches!(
            judge(&readings, Budget::default()),
            Verdict::Within(_)
        ));
    }

    #[test]
    fn thinning_keeps_the_span_and_bounds_the_history() {
        let mut readings: Vec<Reading> = (0..MAX_SAMPLES as u64).map(|s| at(s, s)).collect();
        thin(&mut readings);
        assert!(readings.len() < MAX_SAMPLES);
        assert_eq!(readings.first().map(|r| r.at), Some(Duration::ZERO));
        assert_eq!(
            readings.last().map(|r| r.at),
            Some(Duration::from_secs(MAX_SAMPLES as u64 - 1))
        );
        assert!(readings.windows(2).all(|w| w[0].at < w[1].at));
    }

    #[test]
    fn a_process_that_cannot_be_read_refuses_to_start() {
        fn refuse(_: u32) -> std::io::Result<WriteCounters> {
            Err(std::io::Error::other("no counter"))
        }
        let refused = Sampler::start_with(1, Duration::from_millis(1), refuse);
        assert!(refused.is_err());
    }

    /// The real counter, sampling this process while it writes.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn the_sampler_sees_this_process_write() {
        use std::io::Write;
        let sampler = Sampler::start_with(
            std::process::id(),
            Duration::from_millis(20),
            dc_proc::diskio::write_counters,
        )
        .expect("this process is readable");
        let path = std::env::temp_dir().join(format!("soak-diskio-{}", std::process::id()));
        {
            let mut file = std::fs::File::create(&path).expect("create");
            file.write_all(&vec![1u8; 2 * MIB as usize]).expect("write");
            file.sync_all().expect("sync");
        }
        std::thread::sleep(Duration::from_millis(100));
        let (readings, failure) = sampler.finish();
        let _ = std::fs::remove_file(&path);
        assert!(failure.is_none(), "{failure:?}");
        let (first, last) = (readings.first().unwrap(), readings.last().unwrap());
        assert!(readings.len() >= 3, "{}", readings.len());
        assert!(last.counters.logical_bytes - first.counters.logical_bytes >= 2 * MIB);
    }
}
