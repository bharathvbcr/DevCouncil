//! `rank-eval`: the measurement for "does dense retrieval beat BM25 and
//! learned-sparse here". See `docs/devmap/design/retrieval-eval.md`.
//!
//!     rank-eval corpus  --root DIR --out corpus.txt
//!     rank-eval commits --repo DIR --rev REV --corpus corpus.txt --corpus-root DIR --out q.jsonl
//!     rank-eval dcgrep  --bin dcgrep --root DIR --queries q.jsonl --out run.jsonl
//!     rank-eval fuse    --out run.jsonl [--k 60] a.jsonl b.jsonl
//!     rank-eval score   --queries q.jsonl --corpus corpus.txt --baseline A --arm A=a.jsonl ...

mod commits;
mod corpus;
mod metrics;
mod runs;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use metrics::{Filter, PerQuery};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("rank-eval: {err}");
            ExitCode::from(2)
        }
    }
}

/// `--flag value` pairs; a flag may repeat. Positional arguments are kept.
struct Args {
    flags: BTreeMap<String, Vec<String>>,
    positional: Vec<String>,
}

impl Args {
    fn parse(argv: &[String]) -> Result<Self, String> {
        let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut positional = Vec::new();
        let mut it = argv.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
                flags.entry(name.to_string()).or_default().push(v.clone());
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Self { flags, positional })
    }
    fn one(&self, name: &str) -> Result<&str, String> {
        match self.flags.get(name).map(Vec::as_slice) {
            Some([v]) => Ok(v),
            Some(_) => Err(format!("--{name} given more than once")),
            None => Err(format!("--{name} is required")),
        }
    }
    fn opt(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(|v| v.last()).map(String::as_str)
    }
    fn many(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }
    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.opt(name) {
            None => Ok(default),
            Some(v) => v.parse().map_err(|_| format!("--{name}: {v:?} is not a number")),
        }
    }
}

fn run(argv: Vec<String>) -> Result<(), String> {
    let Some((cmd, rest)) = argv.split_first() else {
        return Err("usage: rank-eval corpus|commits|dcgrep|fuse|score ...".into());
    };
    let args = Args::parse(rest)?;
    match cmd.as_str() {
        "corpus" => cmd_corpus(&args),
        "commits" => cmd_commits(&args),
        "dcgrep" => cmd_dcgrep(&args),
        "fuse" => cmd_fuse(&args),
        "score" => cmd_score(&args),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn read_lines(path: &Path) -> Result<Vec<String>, String> {
    Ok(std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn cmd_corpus(args: &Args) -> Result<(), String> {
    let admitted = corpus::read_slot_files(Path::new(args.one("root")?))?;
    let mut kept: Vec<String> = admitted.iter().filter(|p| corpus::has_source_suffix(p)).cloned().collect();
    kept.sort();
    kept.dedup();
    eprintln!("dcgrep admitted {} files; {} carry a source suffix", admitted.len(), kept.len());
    std::fs::write(args.one("out")?, kept.join("\n") + "\n").map_err(|e| e.to_string())
}

fn cmd_commits(args: &Args) -> Result<(), String> {
    let repo = PathBuf::from(args.one("repo")?);
    let rev = args.one("rev")?;
    let corpus_set: HashSet<String> = read_lines(Path::new(args.one("corpus")?))?.into_iter().collect();
    let corpus_root = PathBuf::from(args.one("corpus-root")?);
    // The text the verbatim check runs against: every file dcgrep admitted,
    // not only the ranked set, lowercased (`rg` is smart-case).
    let mut text = String::new();
    for p in corpus::read_slot_files(&corpus_root)? {
        if let Ok(s) = std::fs::read_to_string(corpus_root.join(&p)) {
            text.push_str(&s.to_lowercase());
            text.push('\u{0}');
        }
    }
    let contains = |q: &str| text.contains(&q.to_lowercase());
    let log = commits::git_log(&repo, rev)?;
    let exists = commits::tree_files(&repo, rev)?;
    let (queries, drops) = commits::select(&log, &exists, &corpus_set, &contains);
    eprintln!("{} non-merge commits reachable from {rev}; {} queries; drops: {drops:?}", log.len(), queries.len());
    let mut out = String::new();
    for q in &queries {
        out.push_str(
            &serde_json::json!({"id": q.id, "query": q.query, "relevant": q.relevant, "source": q.source})
                .to_string(),
        );
        out.push('\n');
    }
    std::fs::write(args.one("out")?, out).map_err(|e| e.to_string())
}

fn cmd_dcgrep(args: &Args) -> Result<(), String> {
    let bin = PathBuf::from(args.one("bin")?);
    let root = PathBuf::from(args.one("root")?);
    let max = args.num("max-results", 5000usize)?;
    let queries = runs::read_queries(Path::new(args.one("queries")?))?;
    let mut rows = Vec::new();
    for q in &queries {
        rows.push((q.id.clone(), runs::dcgrep_rank(&bin, &root, &q.query, max)?));
    }
    runs::write_run(Path::new(args.one("out")?), &rows)
}

fn cmd_fuse(args: &Args) -> Result<(), String> {
    let k = args.num("k", 60.0f64)?;
    if args.positional.len() < 2 {
        return Err("fuse needs at least two run files".into());
    }
    let inputs = args
        .positional
        .iter()
        .map(|p| runs::read_run(Path::new(p)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = Vec::new();
    for id in inputs[0].keys() {
        let lists = inputs
            .iter()
            .map(|r| r.get(id).map(Vec::as_slice).ok_or_else(|| format!("run is missing id {id}")))
            .collect::<Result<Vec<_>, _>>()?;
        rows.push((id.clone(), metrics::rrf(&lists, k)));
    }
    runs::write_run(Path::new(args.one("out")?), &rows)
}

fn cmd_score(args: &Args) -> Result<(), String> {
    let corpus_set: HashSet<String> = read_lines(Path::new(args.one("corpus")?))?.into_iter().collect();
    let excl_prefix = args.many("exclude-prefix");
    let excl_suffix = args.many("exclude-suffix");
    let filter = Filter { corpus: &corpus_set, exclude_prefixes: &excl_prefix, exclude_suffixes: &excl_suffix };
    let resamples = args.num("resamples", 2000usize)?;
    if resamples < 1000 {
        return Err("--resamples must be at least 1000".into());
    }
    let seed = args.num("seed", 20261029u64)?;

    let all = runs::read_queries(Path::new(args.one("queries")?))?;
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for q in all {
        if q.relevant.is_empty() {
            return Err(format!("query {} has no ground truth; label it or remove it", q.id));
        }
        if q.relevant.iter().all(|f| corpus_set.contains(f)) {
            kept.push(q);
        } else {
            dropped.push(q.id);
        }
    }
    if kept.is_empty() {
        return Err("no query has all of its relevant files in the corpus".into());
    }

    let mut arms: Vec<(String, Vec<PerQuery>)> = Vec::new();
    for spec in args.many("arm") {
        let (name, path) = spec.split_once('=').ok_or("--arm wants NAME=run.jsonl")?;
        let run = runs::read_run(Path::new(path))?;
        let mut per = Vec::new();
        for q in &kept {
            let ranked = run
                .get(&q.id)
                .ok_or_else(|| format!("arm {name} has no ranking for query {}; refusing to score it as a miss", q.id))?;
            let rel: HashSet<String> = q.relevant.iter().cloned().collect();
            per.push(metrics::score_query(ranked, &rel, &filter));
        }
        arms.push((name.to_string(), per));
    }
    let baseline = args.one("baseline")?;
    let base = arms
        .iter()
        .find(|(n, _)| n == baseline)
        .ok_or_else(|| format!("--baseline {baseline} is not an --arm"))?
        .1
        .clone();

    let boot = metrics::resample_indices(kept.len(), resamples, seed);
    let label = args.opt("label").unwrap_or("set");
    println!("### {label}: n = {} queries ({} dropped: relevant file outside the corpus), {resamples} resamples, seed {seed}", kept.len(), dropped.len());
    println!();
    println!("| arm | R@1 | R@5 | R@10 | MRR | MRR 95% CI | vs {baseline}: W/L/T | MRR diff | diff 95% CI |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut json_rows = Vec::new();
    for (name, per) in &arms {
        let s = metrics::summarize(per, &boot);
        let p = metrics::paired(per, &base, &boot);
        let versus = if name == baseline {
            ("-".to_string(), "-".to_string(), "-".to_string())
        } else {
            (
                format!("{}/{}/{}", p.wins, p.losses, p.ties),
                format!("{:+.3}", p.diff.mean),
                format!("[{:+.3}, {:+.3}]", p.diff.lo, p.diff.hi),
            )
        };
        println!(
            "| {name} | {:.3} | {:.3} | {:.3} | {:.3} | [{:.3}, {:.3}] | {} | {} | {} |",
            s.recall[0].mean, s.recall[1].mean, s.recall[2].mean, s.mrr.mean, s.mrr.lo, s.mrr.hi,
            versus.0, versus.1, versus.2
        );
        json_rows.push(serde_json::json!({
            "arm": name, "n": s.n,
            "r1": [s.recall[0].mean, s.recall[0].lo, s.recall[0].hi],
            "r5": [s.recall[1].mean, s.recall[1].lo, s.recall[1].hi],
            "r10": [s.recall[2].mean, s.recall[2].lo, s.recall[2].hi],
            "mrr": [s.mrr.mean, s.mrr.lo, s.mrr.hi],
            "vs_baseline": {"baseline": baseline, "wins": p.wins, "losses": p.losses, "ties": p.ties,
                            "mrr_diff": [p.diff.mean, p.diff.lo, p.diff.hi]},
        }));
    }
    println!();
    println!("R@k cells are point estimates; their 95% bootstrap intervals are in the JSON output.");
    if let Some(path) = args.opt("json") {
        let doc = serde_json::json!({"label": label, "n": kept.len(), "dropped": dropped,
            "resamples": resamples, "seed": seed, "arms": json_rows});
        std::fs::write(path, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
