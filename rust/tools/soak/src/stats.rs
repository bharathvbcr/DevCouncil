//! Summaries of the per-build wall times and peak RSS the soak samples, judged
//! against the Medium-repository targets (UNIFIED_RESEARCH_AND_ARCHITECTURE
//! report section 6.6, adopted by the owner on 2026-11-01).
//!
//! Two rules shape this module. A percentile over too few samples is not a
//! percentile: p95 of five builds is the slowest build, so under
//! [`MIN_FOR_P95`] samples the summary prints `not established` and the target
//! line never says `met`. And a target line that could not be judged is never
//! worded like one that was: `met`, `NOT met` and `not established` are three
//! different statements.

use std::fmt::Write as _;

/// One-file incremental build p95 target, Medium repository.
pub const BUILD_P95_TARGET_MS: u64 = 120;
/// Peak resident set target, Medium repository.
pub const PEAK_RSS_TARGET_BYTES: u64 = 512 * 1024 * 1024;
/// The fewest samples at which a 95th percentile is more than the maximum:
/// nearest-rank p95 of N samples is the ceil(0.95 N)-th, which is the very
/// last one for every N below 20.
pub const MIN_FOR_P95: usize = 20;

/// min / median / p95 / max of one series, in the series' own unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub n: usize,
    pub min: u64,
    pub median: u64,
    pub p95: u64,
    pub max: u64,
}

/// Nearest-rank percentile of an ascending, non-empty slice: the
/// `ceil(p/100 * N)`-th value. Integer arithmetic, so no float rounding moves
/// the rank (0.95 * 20 is 19.000000000000004 in f64).
fn nearest_rank(sorted: &[u64], percent: usize) -> u64 {
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// Summarize a series; `None` for an empty one. The median of an even count
/// is the mean of the two middle values, rounded down.
pub fn summarize(values: &[u64]) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        let (a, b) = (u128::from(sorted[n / 2 - 1]), u128::from(sorted[n / 2]));
        ((a + b) / 2) as u64
    };
    Some(Summary {
        n,
        min: sorted[0],
        median,
        p95: nearest_rank(&sorted, 95),
        max: sorted[n - 1],
    })
}

/// Whether a series meets its target, with the reason when it cannot say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Met,
    NotMet,
    /// Fewer than [`MIN_FOR_P95`] samples (or none): neither met nor missed.
    NotEstablished(String),
}

/// `p95 <= target_ms`, but only over enough samples to have a p95.
pub fn judge_p95(summary: Option<&Summary>, target_ms: u64) -> Target {
    match summary {
        None => Target::NotEstablished("no incremental build was measured".into()),
        Some(s) if s.n < MIN_FOR_P95 => Target::NotEstablished(format!(
            "N={} is under the {MIN_FOR_P95} samples a p95 needs; the p95 of {} samples is just their maximum",
            s.n, s.n
        )),
        Some(s) if s.p95 <= target_ms => Target::Met,
        Some(_) => Target::NotMet,
    }
}

/// `max <= target`: a peak is judged on its worst case, which needs no
/// minimum count. An empty series is not established.
pub fn judge_peak(summary: Option<&Summary>, target: u64) -> Target {
    match summary {
        None => Target::NotEstablished("no peak RSS was measured".into()),
        Some(s) if s.max < target => Target::Met,
        Some(_) => Target::NotMet,
    }
}

/// The 1, 5 and 15 minute load averages as `a b c`, from either source the
/// soak reads: macOS `sysctl -n vm.loadavg` (`{ 20.24 20.14 19.47 }`) or
/// Linux `/proc/loadavg` (`0.52 0.58 0.59 1/467 12345`). `None` when the text
/// does not start with three numbers.
pub fn parse_loadavg(text: &str) -> Option<String> {
    let mut fields = text
        .split(|c: char| c.is_whitespace() || c == '{' || c == '}')
        .filter(|f| !f.is_empty());
    let three: Vec<&str> = (0..3).filter_map(|_| fields.next()).collect();
    if three.len() == 3 && three.iter().all(|f| f.parse::<f64>().is_ok()) {
        Some(three.join(" "))
    } else {
        None
    }
}

fn target_word(target: &Target) -> String {
    match target {
        Target::Met => "met".into(),
        Target::NotMet => "NOT met".into(),
        Target::NotEstablished(why) => format!("not established ({why})"),
    }
}

/// The reader-latency line: N, min, median, p95, max in milliseconds. The
/// p95 obeys the same [`MIN_FOR_P95`] rule as the build line: under it the p95
/// is just the maximum, and printing it as a p95 would claim a percentile no
/// sample set that small has. `None` for no samples.
pub fn latency_line(values_ms: &[u64]) -> Option<String> {
    let s = summarize(values_ms)?;
    let p95 = if s.n < MIN_FOR_P95 {
        format!("p95 not established (N={})", s.n)
    } else {
        format!("p95 {} ms", s.p95)
    };
    Some(format!(
        "reader query latency: N={} min {} ms median {} ms {p95} max {} ms",
        s.n, s.min, s.median, s.max
    ))
}

/// The one-file incremental build line: N, min, median, p95, max in
/// milliseconds, the target, and whether it is met.
pub fn build_line(label: &str, values_ms: &[u64]) -> String {
    let Some(s) = summarize(values_ms) else {
        return format!(
            "build {label}: N=0; p95 target {BUILD_P95_TARGET_MS} ms: {}",
            target_word(&judge_p95(None, BUILD_P95_TARGET_MS))
        );
    };
    let p95 = if s.n < MIN_FOR_P95 {
        format!("p95 not established (N={})", s.n)
    } else {
        format!("p95 {} ms", s.p95)
    };
    format!(
        "build {label}: N={} min {} ms median {} ms {p95} max {} ms; p95 target <= {BUILD_P95_TARGET_MS} ms: {}",
        s.n,
        s.min,
        s.median,
        s.max,
        target_word(&judge_p95(Some(&s), BUILD_P95_TARGET_MS))
    )
}

fn mib(bytes: u64) -> String {
    let mut text = String::new();
    let _ = write!(text, "{:.1} MiB", bytes as f64 / (1024.0 * 1024.0));
    text
}

/// The peak-RSS line: min / median / max in MiB against the target.
pub fn rss_line(label: &str, peaks: &[u64]) -> String {
    let target = format!("< {} MiB", PEAK_RSS_TARGET_BYTES >> 20);
    match summarize(peaks) {
        None => format!(
            "rss {label}: N=0; peak target {target}: {}",
            target_word(&judge_peak(None, PEAK_RSS_TARGET_BYTES))
        ),
        Some(s) => format!(
            "rss {label}: N={} min {} median {} max {}; peak target {target}: {}",
            s.n,
            mib(s.min),
            mib(s.median),
            mib(s.max),
            target_word(&judge_peak(Some(&s), PEAK_RSS_TARGET_BYTES))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_p95_takes_the_ceil_rank_not_a_rounded_one() {
        // N=20: ceil(0.95*20) = 19, so the 19th of 1..=20, not the 20th.
        let twenty: Vec<u64> = (1..=20).collect();
        assert_eq!(summarize(&twenty).unwrap().p95, 19);
        // N=25: ceil(23.75) = 24.
        let n25: Vec<u64> = (1..=25).collect();
        assert_eq!(summarize(&n25).unwrap().p95, 24);
        // N=100: exactly 95.
        let n100: Vec<u64> = (1..=100).collect();
        assert_eq!(summarize(&n100).unwrap().p95, 95);
        // Under 20 the p95 is the maximum.
        let five = [30, 10, 50, 20, 40];
        let s = summarize(&five).unwrap();
        assert_eq!((s.p95, s.max, s.min, s.median), (50, 50, 10, 30));
    }

    #[test]
    fn the_input_order_does_not_matter_and_even_medians_average() {
        let s = summarize(&[4, 1, 3, 2]).unwrap();
        assert_eq!((s.min, s.median, s.max), (1, 2, 4));
        assert_eq!(summarize(&[7]).unwrap().p95, 7);
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn a_reader_latency_p95_follows_the_same_minimum_as_the_build_p95() {
        let few: Vec<u64> = (1..=19).collect();
        let line = latency_line(&few).expect("a line");
        assert!(line.contains("p95 not established (N=19)"), "{line}");
        assert!(!line.contains("p95 19"), "{line}");
        assert!(line.contains("N=19 min 1 ms median 10 ms"), "{line}");
        let twenty: Vec<u64> = (1..=20).collect();
        let line = latency_line(&twenty).expect("a line");
        assert!(line.contains("p95 19 "), "{line}");
        assert!(!line.contains("not established"), "{line}");
        assert_eq!(latency_line(&[]), None);
    }

    #[test]
    fn a_p95_over_fewer_than_twenty_samples_is_never_met() {
        // Every sample far under the target: still not "met" at N=19.
        let fast = vec![5u64; 19];
        let s = summarize(&fast);
        assert!(matches!(
            judge_p95(s.as_ref(), 120),
            Target::NotEstablished(_)
        ));
        let line = build_line("restored", &fast);
        assert!(line.contains("p95 not established (N=19)"), "{line}");
        assert!(line.contains("not established"), "{line}");
        assert!(!line.contains(": met"), "{line}");
        // At 20 the same series is judged.
        let twenty = vec![5u64; 20];
        assert_eq!(
            judge_p95(summarize(&twenty).as_ref(), 120),
            Target::Met,
            "N=20 has a p95"
        );
        assert!(build_line("restored", &twenty).ends_with(": met"));
    }

    #[test]
    fn the_target_is_inclusive_and_a_slow_tail_misses_it() {
        // The 24th-ranked of 25 sits exactly on the target.
        let mut at_target = vec![10u64; 25];
        at_target[20..].fill(120);
        assert_eq!(judge_p95(summarize(&at_target).as_ref(), 120), Target::Met);
        // Three slow builds out of 25 put the 24th-ranked value over.
        let mut slow = vec![10u64; 25];
        slow[22] = 121;
        slow[23] = 121;
        slow[24] = 121;
        assert_eq!(judge_p95(summarize(&slow).as_ref(), 120), Target::NotMet);
        assert!(build_line("all", &slow).contains("NOT met"));
        // One outlier out of 25 is the 25th rank: it is not the p95.
        let mut one = vec![10u64; 25];
        one[24] = 5000;
        assert_eq!(judge_p95(summarize(&one).as_ref(), 120), Target::Met);
    }

    #[test]
    fn both_load_average_formats_parse_and_garbage_does_not() {
        assert_eq!(
            parse_loadavg("{ 20.24 20.14 19.47 }\n").as_deref(),
            Some("20.24 20.14 19.47")
        );
        assert_eq!(
            parse_loadavg("0.52 0.58 0.59 1/467 12345\n").as_deref(),
            Some("0.52 0.58 0.59")
        );
        assert_eq!(parse_loadavg("{ 1 2 }"), None);
        assert_eq!(parse_loadavg("{ a b c }"), None);
        assert_eq!(parse_loadavg(""), None);
    }

    #[test]
    fn no_samples_is_not_established_not_met() {
        assert!(matches!(judge_p95(None, 120), Target::NotEstablished(_)));
        assert!(matches!(judge_peak(None, 1), Target::NotEstablished(_)));
        assert!(build_line("x", &[]).contains("N=0"));
        assert!(rss_line("x", &[]).contains("not established"));
    }

    #[test]
    fn the_rss_target_is_strict_and_judged_on_the_worst_case() {
        let under = [400 << 20, 511 << 20];
        assert_eq!(
            judge_peak(summarize(&under).as_ref(), PEAK_RSS_TARGET_BYTES),
            Target::Met
        );
        // Exactly 512 MiB is not "< 512 MiB".
        let at = [PEAK_RSS_TARGET_BYTES];
        assert_eq!(
            judge_peak(summarize(&at).as_ref(), PEAK_RSS_TARGET_BYTES),
            Target::NotMet
        );
        let line = rss_line("incremental", &[100 << 20, 2200 << 20]);
        assert!(line.contains("max 2200.0 MiB"), "{line}");
        assert!(line.contains("NOT met"), "{line}");
    }
}
