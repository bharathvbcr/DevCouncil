//! The one scorer. Every arm is a ranked list of file paths per query id, in
//! one JSONL format, and is scored here, so arms differ only in ranking.
//!
//! * R@k = |relevant in top k| / |relevant|, averaged over queries.
//! * MRR = mean of 1/rank of the first relevant file; 0 when none is ranked.
//! * Intervals are percentile bootstrap over queries. The paired comparison
//!   resamples query indices once per iteration and applies the same indices
//!   to both arms.

use std::collections::{BTreeMap, HashSet};

/// Cutoffs reported for recall.
pub const KS: [usize; 3] = [1, 5, 10];

/// Per-query numbers for one arm.
#[derive(Clone, Debug, PartialEq)]
pub struct PerQuery {
    pub recall: [f64; 3],
    pub rr: f64,
}

/// Which ranked entries count. Entries outside the corpus, or removed by an
/// exclusion, are dropped before ranks are assigned, so a file the corpus does
/// not contain can never push a relevant one down.
pub struct Filter<'a> {
    pub corpus: &'a HashSet<String>,
    pub exclude_prefixes: &'a [String],
    pub exclude_suffixes: &'a [String],
}

impl Filter<'_> {
    pub fn keeps(&self, path: &str) -> bool {
        self.corpus.contains(path)
            && !self.exclude_prefixes.iter().any(|p| path.starts_with(p.as_str()))
            && !self.exclude_suffixes.iter().any(|s| path.ends_with(s.as_str()))
    }
}

/// Score one ranked list. `ranked` may contain duplicates; the first
/// occurrence of a path wins.
pub fn score_query(ranked: &[String], relevant: &HashSet<String>, filter: &Filter) -> PerQuery {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut rank = 0usize;
    let mut first: Option<usize> = None;
    let mut hits_at = [0usize; 3];
    for path in ranked {
        if !filter.keeps(path) || !seen.insert(path.as_str()) {
            continue;
        }
        rank += 1;
        if relevant.contains(path) {
            first.get_or_insert(rank);
            for (slot, k) in KS.iter().enumerate() {
                if rank <= *k {
                    hits_at[slot] += 1;
                }
            }
        }
    }
    let total = relevant.len().max(1) as f64;
    PerQuery {
        recall: [
            hits_at[0] as f64 / total,
            hits_at[1] as f64 / total,
            hits_at[2] as f64 / total,
        ],
        rr: first.map_or(0.0, |r| 1.0 / r as f64),
    }
}

/// splitmix64: small, fixed, and the same on every machine.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` by rejection, so there is no modulo bias.
    pub fn below(&mut self, n: usize) -> usize {
        let n = n as u64;
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let x = self.next_u64();
            if x < zone {
                return (x % n) as usize;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Interval {
    pub mean: f64,
    pub lo: f64,
    pub hi: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn interval(point: f64, mut draws: Vec<f64>) -> Interval {
    draws.sort_by(f64::total_cmp);
    Interval {
        mean: point,
        lo: percentile(&draws, 0.025),
        hi: percentile(&draws, 0.975),
    }
}

/// The bootstrap index matrix, drawn once and shared by every arm and every
/// paired comparison so they are all resampled together.
pub fn resample_indices(n: usize, resamples: usize, seed: u64) -> Vec<Vec<usize>> {
    let mut rng = Rng::new(seed);
    (0..resamples)
        .map(|_| (0..n).map(|_| rng.below(n)).collect())
        .collect()
}

fn mean_over(values: &[f64], idx: &[usize]) -> f64 {
    idx.iter().map(|&i| values[i]).sum::<f64>() / idx.len() as f64
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// One arm's summary over a query set.
#[derive(Clone, Debug)]
pub struct ArmSummary {
    pub n: usize,
    pub recall: [Interval; 3],
    pub mrr: Interval,
}

pub fn summarize(per: &[PerQuery], boot: &[Vec<usize>]) -> ArmSummary {
    let col = |f: &dyn Fn(&PerQuery) -> f64| per.iter().map(f).collect::<Vec<_>>();
    let rr = col(&|p| p.rr);
    let recalls = [
        col(&|p| p.recall[0]),
        col(&|p| p.recall[1]),
        col(&|p| p.recall[2]),
    ];
    let iv = |values: &[f64]| {
        interval(
            mean(values),
            boot.iter().map(|idx| mean_over(values, idx)).collect(),
        )
    };
    ArmSummary {
        n: per.len(),
        recall: [iv(&recalls[0]), iv(&recalls[1]), iv(&recalls[2])],
        mrr: iv(&rr),
    }
}

/// Paired comparison of an arm against the baseline on reciprocal rank.
#[derive(Clone, Debug)]
pub struct Paired {
    pub wins: usize,
    pub losses: usize,
    pub ties: usize,
    /// (arm - baseline) MRR with a paired bootstrap interval.
    pub diff: Interval,
}

pub fn paired(arm: &[PerQuery], base: &[PerQuery], boot: &[Vec<usize>]) -> Paired {
    let d: Vec<f64> = arm.iter().zip(base).map(|(a, b)| a.rr - b.rr).collect();
    let (mut wins, mut losses, mut ties) = (0, 0, 0);
    for v in &d {
        if v.abs() < 1e-12 {
            ties += 1;
        } else if *v > 0.0 {
            wins += 1;
        } else {
            losses += 1;
        }
    }
    Paired {
        wins,
        losses,
        ties,
        diff: interval(mean(&d), boot.iter().map(|idx| mean_over(&d, idx)).collect()),
    }
}

/// Reciprocal-rank fusion of ranked lists: score = sum 1/(k + rank). Ties are
/// broken by path so the output is deterministic.
pub fn rrf(lists: &[&[String]], k: f64) -> Vec<String> {
    let mut score: BTreeMap<&str, f64> = BTreeMap::new();
    for list in lists {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut rank = 0usize;
        for path in *list {
            if !seen.insert(path.as_str()) {
                continue;
            }
            rank += 1;
            *score.entry(path.as_str()).or_insert(0.0) += 1.0 / (k + rank as f64);
        }
    }
    let mut out: Vec<(&str, f64)> = score.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    out.into_iter().map(|(p, _)| p.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rank_is_assigned_after_the_filter() {
        let corpus = set(&["a", "b", "c"]);
        let f = Filter { corpus: &corpus, exclude_prefixes: &[], exclude_suffixes: &[] };
        // "zzz" is outside the corpus and must not take rank 1.
        let q = score_query(&list(&["zzz", "b", "a"]), &set(&["b"]), &f);
        assert_eq!(q.rr, 1.0);
        assert_eq!(q.recall, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn recall_is_a_fraction_of_the_relevant_set_and_duplicates_count_once() {
        let corpus = set(&["a", "b", "c", "d"]);
        let f = Filter { corpus: &corpus, exclude_prefixes: &[], exclude_suffixes: &[] };
        let q = score_query(&list(&["a", "a", "c", "b"]), &set(&["b", "c"]), &f);
        // ranks: a=1 c=2 b=3
        assert_eq!(q.rr, 0.5);
        assert_eq!(q.recall[0], 0.0);
        assert_eq!(q.recall[1], 1.0);
    }

    #[test]
    fn exclusions_remove_entries_before_ranking() {
        let corpus = set(&["docs/x.md", "src/a.rs", "src/b.rs"]);
        let ex = vec!["docs/".to_string()];
        let f = Filter { corpus: &corpus, exclude_prefixes: &ex, exclude_suffixes: &[] };
        let q = score_query(&list(&["docs/x.md", "src/b.rs", "src/a.rs"]), &set(&["src/a.rs"]), &f);
        assert_eq!(q.rr, 0.5);
    }

    #[test]
    fn an_unranked_relevant_file_scores_zero() {
        let corpus = set(&["a", "b"]);
        let f = Filter { corpus: &corpus, exclude_prefixes: &[], exclude_suffixes: &[] };
        let q = score_query(&list(&["a"]), &set(&["b"]), &f);
        assert_eq!(q.rr, 0.0);
        assert_eq!(q.recall, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_bootstrap_is_reproducible_and_paired_interval_excludes_zero_for_a_clear_win() {
        let a: Vec<PerQuery> = (0..60).map(|_| PerQuery { recall: [1.0; 3], rr: 1.0 }).collect();
        let b: Vec<PerQuery> = (0..60).map(|_| PerQuery { recall: [0.0; 3], rr: 0.1 }).collect();
        let boot = resample_indices(60, 1000, 7);
        assert_eq!(boot, resample_indices(60, 1000, 7));
        let p = paired(&a, &b, &boot);
        assert_eq!((p.wins, p.losses, p.ties), (60, 0, 0));
        assert!(p.diff.lo > 0.0);
    }

    #[test]
    fn identical_arms_tie_and_the_interval_is_zero() {
        let a: Vec<PerQuery> = (0..30)
            .map(|i| PerQuery { recall: [0.0; 3], rr: 1.0 / (1 + i % 4) as f64 })
            .collect();
        let boot = resample_indices(30, 1000, 1);
        let p = paired(&a, &a, &boot);
        assert_eq!(p.ties, 30);
        assert_eq!((p.diff.lo, p.diff.hi), (0.0, 0.0));
    }

    #[test]
    fn rrf_rewards_agreement_and_is_deterministic() {
        let one = list(&["a", "b", "c"]);
        let two = list(&["b", "c", "d"]);
        let fused = rrf(&[&one, &two], 60.0);
        // b is rank 2 and 1, c is rank 3 and 2, a is rank 1 and absent.
        assert_eq!(fused[0], "b");
        assert_eq!(fused, rrf(&[&one, &two], 60.0));
        assert_eq!(fused.len(), 4);
    }

    #[test]
    fn rejection_sampling_stays_in_range() {
        let mut r = Rng::new(3);
        assert!((0..1000).all(|_| r.below(7) < 7));
    }
}
