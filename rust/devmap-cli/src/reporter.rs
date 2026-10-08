//! The build's stage reporter: what `build` shows while it runs, and the timing
//! breakdown it attributes to each stage.

use std::time::Instant;

use crate::cli::ProgressMode;
use crate::progress;

/// One completed span and the spans that ran inside it.
///
/// Recursive, because the breakdown is. A stage contains sub-phases, and a
/// sub-phase contains the split its own implementation measured — the
/// generation write reports what each relation cost, and only the store can,
/// since the node and full-text inserts are one interleaved loop. Two levels
/// were enough while `persist:write` was one number; a third would have needed
/// a second, near-identical struct, and the rule below is the same at every
/// depth.
pub(crate) struct StageTiming {
    label: String,
    seconds: f64,
    /// Spans closed while this one was open. Their durations are *included* in
    /// `seconds`; they break it down, they do not add to it. Summing two levels
    /// would double-count the build.
    sub: Vec<StageTiming>,
}

/// The stage in flight: its label, when it began, and the sub-phases closed
/// inside it so far.
///
/// Named rather than written inline because the tuple appears in a field, a
/// borrow and two closures, and a reader meeting `(String, Instant, Vec<(String,
/// f64)>)` in any of them has to reconstruct which position means what.
type OpenStage = (String, Instant, Vec<StageTiming>);

pub(crate) struct ProgressReporter {
    pub(crate) display: progress::Display,
    pub(crate) started_at: Instant,
    json: bool,
    /// The stage currently running: its label, when it began, and the
    /// sub-phases closed inside it so far.
    ///
    /// A stage's cost is recorded when the stage *ends*, against its own label.
    /// Attributing it at the next stage boundary — which is what this reporter
    /// used to do — shifts every measurement one position: on a 13.44 s
    /// scholarlm build, extraction's 7.25 s was printed beside the word
    /// "resolving" and extraction itself was reported as 42 ns. A profiler that
    /// names the wrong phase is worse than none, because the reader acts on it.
    ///
    /// A cumulative-only readout ("complete in 2.90s") is the other failure:
    /// it cannot tell an operator whether a slow build is parsing, resolving,
    /// or writing, and those have nothing in common as fixes.
    ///
    /// `RefCell` because `stage` takes `&self`: the reporter is shared by the
    /// whole pipeline and must not need a mutable borrow to print.
    pub(crate) open: std::cell::RefCell<Option<OpenStage>>,
    /// Stages that have closed, in the order they ran, for `--json` builds.
    /// Kept so a benchmark or a daemon can consume the breakdown without
    /// scraping stderr, which is formatted for humans and not a contract.
    timings: std::cell::RefCell<Vec<StageTiming>>,
}

impl ProgressReporter {
    const TOTAL_STAGES: usize = progress::TOTAL_STAGES;

    pub(crate) fn new(mode: ProgressMode, json: bool, verbose: bool) -> Self {
        Self {
            display: progress::Display::new(mode, json, verbose),
            started_at: Instant::now(),
            json,
            open: std::cell::RefCell::new(None),
            timings: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Close the running stage, recording its cost against its own label.
    pub(crate) fn close_open_stage(&self, succeeded: bool) {
        let Some((label, started, sub)) = self.open.borrow_mut().take() else {
            return;
        };
        let seconds = started.elapsed().as_secs_f64();
        self.display.closed(&label, seconds, succeeded);
        self.timings.borrow_mut().push(StageTiming {
            label,
            seconds,
            sub,
        });
    }

    /// Record and announce a stage boundary.
    ///
    /// Timings are recorded whether or not printing is enabled: `--progress
    /// never` is about keeping stderr clean, not about declining to measure,
    /// and the `--json` breakdown must not depend on the human output being on.
    pub(crate) fn stage(&self, current: usize, message: impl std::fmt::Display) {
        self.close_open_stage(true);
        let rendered = message.to_string();
        self.display.stage(current, &rendered);
        *self.open.borrow_mut() = Some((rendered, Instant::now(), Vec::new()));
    }

    /// Time one sub-phase, recording it under `label` without printing a stage
    /// header. Used to break a stage that is too coarse to act on into the
    /// parts that have different fixes.
    ///
    /// The result is returned untouched, including the error case: a phase that
    /// fails is still a phase that took time, and swallowing the error to keep
    /// the timing tidy would trade a correct build for a pretty number.
    pub(crate) fn timed<T, E>(
        &self,
        label: &str,
        work: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        self.display.detail(label);
        let started = Instant::now();
        let outcome = work();
        let elapsed = started.elapsed().as_secs_f64();
        self.display.phase(label, elapsed);
        self.display.detail("");
        self.record(StageTiming {
            label: label.to_string(),
            seconds: elapsed,
            sub: Vec::new(),
        });
        outcome
    }

    /// Time one sub-phase whose implementation reports its own split.
    ///
    /// Some phases can only be broken down from the inside. `persist:write` is
    /// the case that forced this: it is 0.30 s of a 1.10 s one-file incremental
    /// build on this repository, the relations under it have nothing in common
    /// as fixes, and the node and full-text writes are one interleaved loop
    /// that nothing outside the store can separate. The store measures them and
    /// hands the labelled spans back here.
    ///
    /// The parts nest inside the sub-phase and are already counted in its
    /// `seconds`, exactly as sub-phases are counted in their stage's.
    pub(crate) fn timed_split<T, E>(
        &self,
        label: &str,
        work: impl FnOnce() -> std::result::Result<(T, Vec<(String, f64)>), E>,
    ) -> std::result::Result<T, E> {
        self.display.detail(label);
        let started = Instant::now();
        let outcome = work();
        let elapsed = started.elapsed().as_secs_f64();
        self.display.phase(label, elapsed);
        self.display.detail("");
        // An error path reports the phase with no split rather than no phase:
        // a write that failed halfway still took the time, and the parts it
        // managed to charge are not a breakdown of what it did.
        let parts = match &outcome {
            Ok((_, parts)) => parts.clone(),
            Err(_) => Vec::new(),
        };
        self.record(StageTiming {
            label: label.to_string(),
            seconds: elapsed,
            sub: parts
                .into_iter()
                .map(|(label, seconds)| StageTiming {
                    label,
                    seconds,
                    sub: Vec::new(),
                })
                .collect(),
        });
        outcome.map(|(value, _)| value)
    }

    /// File a closed span under the stage that was open when it ran.
    ///
    /// One owner for that decision, so `timed` and `timed_split` cannot come to
    /// disagree about where a sub-phase lands. A span closed outside any stage
    /// becomes a stage of its own rather than being dropped: silence there is
    /// how a phase goes unattributed and its time is charged to nothing.
    fn record(&self, timing: StageTiming) {
        match self.open.borrow_mut().as_mut() {
            Some((_, _, sub)) => sub.push(timing),
            None => self.timings.borrow_mut().push(timing),
        }
    }

    /// An untimed detail line under the current stage. Does not disturb the
    /// stage clock, so a note between two phases cannot be mistaken for one.
    pub(crate) fn note(&self, message: impl std::fmt::Display) {
        self.display.note(message);
    }

    /// Close the last stage and print the total.
    ///
    /// Deliberately not a `stage` call: completion is an instant, not a span,
    /// and opening a fifth stage here would leave it running forever and put a
    /// zero-length entry in the breakdown.
    pub(crate) fn complete(&self, generation_id: u32) {
        self.close_open_stage(true);
        if !self.json {
            self.display.finish("");
            return;
        }
        self.display.finish(format_args!(
            "[{}/{}] complete: generation #{generation_id} in {}",
            Self::TOTAL_STAGES,
            Self::TOTAL_STAGES,
            progress::duration(self.started_at.elapsed().as_secs_f64())
        ));
    }

    pub(crate) fn up_to_date(&self, generation: u32, files: usize) {
        self.close_open_stage(true);
        if !self.json {
            self.display.finish("");
            return;
        }
        self.display.finish(format_args!(
            "up to date: generation #{generation} · {} checked in {} · resolve/analyze/write skipped",
            progress::count(files, "file"),
            progress::duration(self.started_at.elapsed().as_secs_f64())
        ));
    }

    /// The recorded breakdown as `{stage_label: seconds}` plus the total, for
    /// embedding in a `--json` build result.
    /// The recorded breakdown, for embedding in a `--json` build result.
    ///
    /// Sub-phase seconds are nested inside their stage and are already counted
    /// in the stage's own `seconds`; a consumer sums one level, never both.
    /// A stage still running when this is called is reported with its elapsed
    /// time so far and `"open": true`, because omitting it would make the
    /// stages silently fail to account for the total.
    pub(crate) fn timings_json(&self) -> serde_json::Value {
        fn render(label: &str, secs: f64, sub: &[StageTiming], open: bool) -> serde_json::Value {
            let mut entry = serde_json::json!({"stage": label, "seconds": secs});
            if !sub.is_empty() {
                entry["sub"] = sub
                    .iter()
                    .map(|t| render(&t.label, t.seconds, &t.sub, false))
                    .collect();
            }
            if open {
                entry["open"] = serde_json::Value::Bool(true);
            }
            entry
        }
        let mut stages: Vec<serde_json::Value> = self
            .timings
            .borrow()
            .iter()
            .map(|t| render(&t.label, t.seconds, &t.sub, false))
            .collect();
        if let Some((label, started, sub)) = self.open.borrow().as_ref() {
            stages.push(render(label, started.elapsed().as_secs_f64(), sub, true));
        }
        serde_json::json!({
            "stages": stages,
            "total_seconds": self.started_at.elapsed().as_secs_f64(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Class E (PLAN.md §3.1): a measurement is attributed to what incurred it.
    ///
    /// K8 recorded time-since-previous-announcement against the *next* stage's
    /// label, so a 13.44 s build reported extraction's 7.25 s beside the word
    /// "resolving" and extraction itself as 42 nanoseconds. The output was
    /// plausible — real phase names, real durations, summing to the real total
    /// — and pointed at the wrong one, which is what makes this class corrosive
    /// rather than merely wrong.
    ///
    /// The three properties below are what a consumer needs in order to trust
    /// the breakdown, and none of them held before the fix.
    #[test]
    fn stage_timings_are_attributed_to_the_stage_that_incurred_them() {
        let reporter = ProgressReporter::new(ProgressMode::Never, true, false);

        reporter.stage(1, "alpha");
        std::thread::sleep(std::time::Duration::from_millis(20));
        reporter.stage(2, "beta");
        let _: std::result::Result<(), ()> = reporter.timed("beta:inner", || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            Ok(())
        });
        reporter.complete(1);

        let json = reporter.timings_json();
        let stages = json["stages"].as_array().expect("stages array");
        assert_eq!(stages.len(), 2, "two stages ran: {stages:?}");

        // 1. The stage that slept is the stage that reports the time. Under the
        //    off-by-one, `alpha`'s 20 ms was reported against `beta`.
        assert_eq!(stages[0]["stage"], "alpha");
        let alpha = stages[0]["seconds"].as_f64().expect("alpha seconds");
        assert!(
            alpha >= 0.015,
            "alpha slept 20ms and must report it, got {alpha}s"
        );

        // 2. Sub-phases nest inside their parent and are included in its total,
        //    never listed beside it — summing both levels would double-count.
        let beta = stages[1]["seconds"].as_f64().expect("beta seconds");
        let sub = stages[1]["sub"].as_array().expect("beta sub-phases");
        assert_eq!(sub.len(), 1, "one sub-phase ran inside beta: {sub:?}");
        assert_eq!(sub[0]["stage"], "beta:inner");
        let inner = sub[0]["seconds"].as_f64().expect("inner seconds");
        assert!(
            inner <= beta + 1e-6,
            "a sub-phase cannot exceed the stage containing it: {inner}s in {beta}s"
        );

        // 3. The stages account for the whole build. A phase that went
        //    unattributed would leave a gap here, which is exactly how a
        //    42-nanosecond extraction went unnoticed.
        let total = json["total_seconds"].as_f64().expect("total");
        assert!(
            alpha + beta <= total + 1e-6 && alpha + beta >= total * 0.5,
            "stages ({alpha}s + {beta}s) must account for the total ({total}s)"
        );
    }

    /// A stage still running is reported as open, never omitted.
    ///
    /// Class A applied to the profiler itself: if an unfinished stage were
    /// simply left out, the breakdown would silently fail to account for the
    /// total and a reader would attribute the missing time to nothing at all.
    #[test]
    fn an_unfinished_stage_is_reported_rather_than_dropped() {
        let reporter = ProgressReporter::new(ProgressMode::Never, true, false);
        reporter.stage(1, "still running");

        let json = reporter.timings_json();
        let stages = json["stages"].as_array().expect("stages array");
        assert_eq!(stages.len(), 1, "the open stage must appear: {stages:?}");
        assert_eq!(stages[0]["stage"], "still running");
        assert_eq!(
            stages[0]["open"],
            serde_json::Value::Bool(true),
            "an unfinished stage must be marked open so its time is not read as final"
        );
    }
}
