//! A resolution baseline measured on a real repository, not a fixture.
//!
//! The labelled fixtures are written to exercise one mechanism each, so they
//! say whether a rung works and nothing about how often it is the right rung
//! on code nobody wrote for the resolver. This test answers that for one real
//! corpus: MarkDev (Swift, Rust, Python), pinned to a commit, with a
//! stratified sample of call sites whose targets were labelled by reading the
//! source — blind to what the resolver answered, so the labels cannot echo it.
//!
//! **Precision** is correct bindings over bindings made; **recall** is correct
//! bindings over sampled sites whose target is declared in the corpus. Both
//! are carried with their counts, because 40 sites is a sample and a
//! percentage alone would hide that.
//!
//! `#[ignore]`d: it needs a MarkDev checkout at the pinned commit. Run with
//!
//! ```text
//! DEVMAP_MARKDEV_ROOT=/path/to/MarkDev \
//!   cargo test -p devmap-resolve --test real_corpus_resolution -- --ignored --nocapture
//! ```
//!
//! A checkout at any other commit is refused, not measured: the sample's line
//! numbers and the corpus-wide uniqueness the resolver leans on both belong to
//! the pinned tree.

use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::Resolver;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn sample_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata/realcorpus/markdev_resolution_sample.json")
}

/// `HEAD` of a checkout, read from `.git` without spawning git.
///
/// Handles a detached HEAD, a loose ref, a packed ref, and a linked worktree
/// (`.git` is a file naming the real git dir, whose `commondir` holds refs).
fn head_commit(root: &Path) -> String {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_file() {
        let pointer = std::fs::read_to_string(&dot_git).expect(".git file is readable");
        let target = pointer
            .trim()
            .strip_prefix("gitdir: ")
            .expect(".git file names its git dir");
        root.join(target)
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).expect("HEAD is readable");
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ") else {
        return head.to_string();
    };
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .map(|dir| git_dir.join(dir.trim()))
        .unwrap_or_else(|_| git_dir.clone());
    for dir in [&git_dir, &common] {
        if let Ok(sha) = std::fs::read_to_string(dir.join(reference)) {
            return sha.trim().to_string();
        }
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    packed
        .lines()
        .find_map(|line| {
            let (sha, name) = line.split_once(' ')?;
            (name == reference).then(|| sha.to_string())
        })
        .unwrap_or_else(|| panic!("{reference} is neither a loose nor a packed ref"))
}

struct Site {
    file: String,
    line: usize,
    caller: String,
    callee: String,
    receiver: String,
    /// `Some(file::name)` when the callee is declared in the corpus; `None`
    /// when it is external (std, a framework, a crate) or names no
    /// declaration (a closure-typed value).
    truth: Option<String>,
}

fn load_sample() -> (serde_json::Value, Vec<Site>) {
    let raw = std::fs::read_to_string(sample_path()).expect("sample is readable");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("sample is JSON");
    let sites = value["sites"]
        .as_array()
        .expect("sites is an array")
        .iter()
        .map(|site| Site {
            file: site["file"].as_str().expect("file").to_string(),
            line: site["line"].as_u64().expect("line") as usize,
            caller: site["caller"].as_str().expect("caller").to_string(),
            callee: site["callee"].as_str().expect("callee").to_string(),
            receiver: site["receiver"].as_str().unwrap_or("").to_string(),
            truth: site["truth"].as_str().map(str::to_string),
        })
        .collect();
    (value, sites)
}

/// The final name segment of a symbol id: `a.rs::Foo.new` -> `new`.
fn last_segment(symbol: &str) -> &str {
    let tail = symbol.rsplit("::").next().unwrap_or(symbol);
    tail.rsplit('.').next().unwrap_or(tail)
}

/// Confirm the sampled call is still where the sample says it is, alone.
///
/// The resolver's edges carry no call span, so a site is matched to its
/// answer by `(caller, callee name)`. That join is only sound when the caller
/// makes exactly one call by that name — the sample was drawn under that rule,
/// and this re-checks it rather than trusting it.
fn check_site_shape(extraction: &Extraction, site: &Site) -> Result<(), String> {
    let source = extraction.source_code.as_deref().unwrap_or("");
    let same_name: Vec<_> = extraction
        .calls
        .iter()
        .filter(|call| {
            // A call at file scope has no caller; the sample spells that "".
            call.caller_symbol.as_deref().unwrap_or("") == site.caller
                && call.callee_name == site.callee
        })
        .collect();
    if same_name.len() != 1 {
        return Err(format!(
            "{} calls `{}` {} times, so its edges cannot say which call they answer",
            site.caller,
            site.callee,
            same_name.len()
        ));
    }
    let call = same_name[0];
    let start = call.span.start_byte.min(source.len());
    let line = source[..start].matches('\n').count() + 1;
    let receiver = call.receiver_expr.as_deref().unwrap_or("");
    if line != site.line || receiver != site.receiver {
        return Err(format!(
            "`{}` in {} is at line {line} with receiver `{receiver}`, the sample says \
             line {} with `{}`",
            site.callee, site.caller, site.line, site.receiver
        ));
    }
    Ok(())
}

#[derive(Default)]
struct Tally {
    bound: usize,
    correct: usize,
    wrong: usize,
    missed: usize,
    abstained_external: usize,
    in_corpus: usize,
}

#[test]
#[ignore = "needs DEVMAP_MARKDEV_ROOT: a MarkDev checkout at the pinned commit"]
fn markdev_resolution_has_not_regressed() {
    let Some(root) = std::env::var_os("DEVMAP_MARKDEV_ROOT").map(PathBuf::from) else {
        // Asked for by `--ignored` and unable to run: that is a failure, not
        // a pass. A baseline that reports green when it measured nothing is
        // exactly how "approved" comes to mean "unexamined".
        panic!(
            "DEVMAP_MARKDEV_ROOT is unset, so nothing was measured. Point it at a \
             MarkDev checkout of the commit pinned in {}",
            sample_path().display()
        );
    };
    let (sample, sites) = load_sample();
    let pinned = sample["pinned_commit"].as_str().expect("pinned_commit");
    let head = head_commit(&root);
    assert_eq!(
        head,
        pinned,
        "{} is at {head}, the sample is labelled against {pinned}. Check out the \
         pinned commit (a `git worktree add <dir> {pinned}` is enough); a \
         different tree would be measured against labels that describe another one",
        root.display()
    );

    let (extractions, _report) =
        devmap_extract::extract_tree_with_report(&root).expect("MarkDev extracts");
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver
        .resolve_all(&extractions)
        .expect("MarkDev resolves");

    // caller -> callee name -> every target a Calls edge binds it to.
    let mut answers: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for edge in &resolution.edges {
        if edge.edge_kind != EdgeKind::Calls {
            continue;
        }
        answers
            .entry((
                edge.source_symbol.as_str(),
                last_segment(&edge.target_symbol),
            ))
            .or_default()
            .insert(edge.target_symbol.as_str());
    }
    let by_file: BTreeMap<&str, &Extraction> = extractions
        .iter()
        .map(|extraction| (extraction.file_path.as_str(), extraction))
        .collect();

    let mut tally = Tally::default();
    let mut drift: Vec<String> = Vec::new();
    let mut rows: Vec<String> = Vec::new();
    for site in &sites {
        let Some(extraction) = by_file.get(site.file.as_str()) else {
            drift.push(format!("{}: not extracted", site.file));
            continue;
        };
        if let Err(why) = check_site_shape(extraction, site) {
            drift.push(format!("{}:{}: {why}", site.file, site.line));
            continue;
        }
        let got = answers
            .get(&(site.caller.as_str(), site.callee.as_str()))
            .cloned()
            .unwrap_or_default();
        if site.truth.is_some() {
            tally.in_corpus += 1;
        }
        let verdict = match (&site.truth, got.is_empty()) {
            (Some(_), true) => {
                tally.missed += 1;
                "missed"
            }
            (None, true) => {
                tally.abstained_external += 1;
                "abstained"
            }
            (truth, false) => {
                tally.bound += 1;
                // An ambiguous binding — two targets for one call — is right
                // only if it is not ambiguous: a reader handed both cannot act.
                let right = truth
                    .as_deref()
                    .is_some_and(|want| got.len() == 1 && got.contains(want));
                if right {
                    tally.correct += 1;
                    "correct"
                } else {
                    tally.wrong += 1;
                    "WRONG"
                }
            }
        };
        rows.push(format!(
            "  {verdict:9} {}:{} {} -> want {} got {:?}",
            site.file,
            site.line,
            site.callee,
            site.truth.as_deref().unwrap_or("(external/none)"),
            got
        ));
    }

    assert!(
        drift.is_empty(),
        "the corpus no longer matches the sample, so its numbers would describe \
         a different tree:\n  {}",
        drift.join("\n  ")
    );

    let precision = tally.correct * 1000 / tally.bound.max(1);
    let recall = tally.correct * 1000 / tally.in_corpus.max(1);
    println!(
        "\nMarkDev resolution sample ({} sites at {pinned}):",
        sites.len()
    );
    println!("{}", rows.join("\n"));
    println!(
        "  bound {} (correct {}, wrong {}), missed {}, abstained on external {}\n  \
         precision {}/{} = {precision} permille, recall {}/{} = {recall} permille\n",
        tally.bound,
        tally.correct,
        tally.wrong,
        tally.missed,
        tally.abstained_external,
        tally.correct,
        tally.bound,
        tally.correct,
        tally.in_corpus,
    );

    // A ratchet on the committed counts. Raising them is a reviewed edit to
    // the sample's `baseline`; falling below them is a regression to explain.
    let baseline = &sample["baseline"];
    let floor_precision = baseline["precision_permille"].as_u64().expect("precision") as usize;
    let floor_recall = baseline["recall_permille"].as_u64().expect("recall") as usize;
    assert!(
        precision >= floor_precision && recall >= floor_recall,
        "MarkDev resolution regressed: precision {precision} (floor \
         {floor_precision}), recall {recall} (floor {floor_recall}) permille"
    );
}
