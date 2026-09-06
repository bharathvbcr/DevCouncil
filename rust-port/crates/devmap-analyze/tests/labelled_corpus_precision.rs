//! W4.1 — a precision number for the `extracted` tier, and a defence of it.
//!
//! The 40 golden fixtures pin *identity*: same nodes, same edges as a frozen
//! Python implementation that has since been retired. Identity is not
//! correctness, and nothing anywhere answered the only question that matters
//! about a dead-code tool: **of the symbols we called dead, how many were?**
//!
//! `testdata/golden/<fixture>/truth.json` answers it. Every symbol in six
//! fixtures is hand-labelled `live` or not, each with the evidence, so the
//! numbers below are measured against a reviewed answer rather than against a
//! previous run of the same code.
//!
//! **The fence is precision at `extracted`, at 1.0.** That tier's contract is
//! "safe to act on"; a false positive in it is not a metric dip but a contract
//! violation, and acting on one deletes live code. `inferred` and `ambiguous`
//! claim less and are reported, not fenced.
//!
//! Recall is reported too, and one fixture (`liveness_truth`) exists to make it
//! measurable — the other five are all-live by construction, which measures
//! precision and says nothing about what was missed.

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Tier boundaries, matching the kernel's own `confidence_millis` ladder.
const EXTRACTED_FLOOR: f32 = 0.9;
const INFERRED_FLOOR: f32 = 0.4;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> sits two levels under the workspace root")
        .to_path_buf()
}

#[derive(Debug)]
struct Truth {
    fixture: String,
    source: PathBuf,
    /// `symbol_id -> live`
    labels: BTreeMap<String, bool>,
}

fn load_truth() -> Vec<Truth> {
    let golden = workspace_root().join("testdata/golden");
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&golden)
        .expect("testdata/golden is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for dir in entries {
        let truth_path = dir.join("truth.json");
        if !truth_path.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&truth_path).expect("truth.json is readable");
        let value: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("{}: {error}", truth_path.display()));
        let labels = value["symbols"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: symbols is not an array", truth_path.display()))
            .iter()
            .map(|entry| {
                (
                    entry["symbol_id"].as_str().expect("symbol_id").to_string(),
                    entry["live"].as_bool().expect("live"),
                )
            })
            .collect();
        out.push(Truth {
            fixture: dir.file_name().unwrap().to_string_lossy().into_owned(),
            source: workspace_root().join(value["source"].as_str().expect("source")),
            labels,
        });
    }
    assert!(!out.is_empty(), "no truth.json found under testdata/golden");
    out
}

/// Extract every file of a fixture, in a stable order.
fn extract_fixture(source: &Path) -> Vec<Extraction> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(source)
        .unwrap_or_else(|error| panic!("{}: {error}", source.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let rel = path
                .strip_prefix(source)
                .expect("path is under the fixture")
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            extract_file(&rel, &text)
        })
        .collect()
}

/// What the kernel claims is dead, as `symbol_id -> confidence`.
///
/// Both lists, together. `dead_clusters` is where the abandoned-cycle shape
/// lands — every member has an inbound edge from another member, so the
/// one-hop join cannot see it — and scoring only `dead_symbols` would credit
/// the kernel with a recall it does not have and charge it for a miss it
/// already reports elsewhere.
fn kernel_verdicts(extractions: &[Extraction]) -> BTreeMap<String, f32> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = analyze(extractions, &resolution);

    let mut claimed = BTreeMap::new();
    for report in &analysis.dead_symbols {
        if report.is_exempt {
            continue;
        }
        claimed.insert(
            format!("{}::{}", report.file_path, report.symbol_name),
            report.confidence,
        );
    }
    for cluster in &analysis.dead_clusters.clusters {
        for member in &cluster.members {
            claimed.entry(member.clone()).or_insert(cluster.confidence);
        }
    }
    claimed
}

fn tier(confidence: f32) -> &'static str {
    if confidence >= EXTRACTED_FLOOR {
        "extracted"
    } else if confidence >= INFERRED_FLOOR {
        "inferred"
    } else {
        "ambiguous"
    }
}

#[derive(Default, Debug, Clone, Copy)]
struct Score {
    true_positive: usize,
    false_positive: usize,
}

/// The fence, and the whole reason this file exists.
///
/// A single false positive here fails the build. That is the same strictness
/// the existing honesty invariants carry, and for the same reason: `extracted`
/// is the tier `CLAUDE.md` tells agents to act on without further checking.
#[test]
fn the_extracted_tier_has_no_false_positives() {
    let mut by_tier: BTreeMap<&str, Score> = BTreeMap::new();
    let mut violations: Vec<String> = Vec::new();

    for truth in load_truth() {
        let extractions = extract_fixture(&truth.source);
        let claimed = kernel_verdicts(&extractions);

        for (symbol_id, confidence) in &claimed {
            // A claim about a symbol nobody labelled is itself a finding: the
            // corpus is meant to be exhaustive, so an unlabelled id means the
            // fixture drifted from its truth file and the score below would be
            // computed over a corpus that no longer matches.
            let Some(&live) = truth.labels.get(symbol_id) else {
                violations.push(format!(
                    "{}: kernel called `{symbol_id}` dead, but truth.json does not \
                     label it — the fixture and its labels have drifted apart",
                    truth.fixture
                ));
                continue;
            };
            let entry = by_tier.entry(tier(*confidence)).or_default();
            if live {
                entry.false_positive += 1;
                violations.push(format!(
                    "{}: `{symbol_id}` is live but was called dead at {} ({confidence:.2})",
                    truth.fixture,
                    tier(*confidence),
                ));
            } else {
                entry.true_positive += 1;
            }
        }
    }

    let extracted = by_tier.get("extracted").copied().unwrap_or_default();
    let extracted_claims = extracted.true_positive + extracted.false_positive;
    assert!(
        extracted_claims > 0,
        "no `extracted`-tier claims were scored, so a precision of 1.0 would be \
         vacuous — the corpus must contain at least one confidently dead symbol"
    );

    let extracted_violations: Vec<&String> = violations
        .iter()
        .filter(|line| line.contains("at extracted") || line.contains("drifted apart"))
        .collect();
    assert!(
        extracted_violations.is_empty(),
        "`extracted` precision is {:.3}, not 1.0. That tier means `safe to act \
         on`, so every line below is a symbol this kernel would have told \
         somebody to delete:\n  {}",
        extracted.true_positive as f64 / extracted_claims as f64,
        extracted_violations
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// The numbers, printed rather than fenced.
///
/// `inferred` and `ambiguous` claim less than `extracted` and are allowed to be
/// wrong; what they are not allowed to do is be wrong silently. Run with
/// `--nocapture` to read the table.
#[test]
fn the_corpus_reports_precision_and_recall_per_tier() {
    let mut rows: Vec<String> = Vec::new();
    let mut total_dead = 0usize;
    let mut total_found = 0usize;

    for truth in load_truth() {
        let extractions = extract_fixture(&truth.source);
        let claimed = kernel_verdicts(&extractions);
        let labelled_dead: Vec<&String> = truth
            .labels
            .iter()
            .filter(|(_, live)| !**live)
            .map(|(id, _)| id)
            .collect();
        let found = labelled_dead
            .iter()
            .filter(|id| claimed.contains_key(**id))
            .count();
        total_dead += labelled_dead.len();
        total_found += found;

        let correct = claimed
            .iter()
            .filter(|(id, _)| truth.labels.get(*id) == Some(&false))
            .count();
        let precision = if claimed.is_empty() {
            // Not 1.0. A tool that claims nothing has not been shown to be
            // precise; it has been shown to be silent, and rendering that as a
            // perfect score is the same "absence read as evidence" this whole
            // corpus exists to catch.
            "n/a (no claims)".to_string()
        } else {
            format!("{:.3}", correct as f64 / claimed.len() as f64)
        };
        let recall = if labelled_dead.is_empty() {
            "n/a (nothing dead)".to_string()
        } else {
            format!("{:.3}", found as f64 / labelled_dead.len() as f64)
        };
        rows.push(format!(
            "  {:16} labelled={:2} dead={:2} claimed={:2} precision={:16} recall={}",
            truth.fixture,
            truth.labels.len(),
            labelled_dead.len(),
            claimed.len(),
            precision,
            recall
        ));
    }

    println!("\nW4.1 labelled corpus:\n{}", rows.join("\n"));
    println!(
        "  {:16} dead symbols labelled: {total_dead}, found: {total_found} \
         (recall {:.3})\n",
        "TOTAL",
        total_found as f64 / total_dead.max(1) as f64
    );

    assert!(
        total_dead > 0,
        "no fixture labels anything dead, so recall is unmeasurable and the \
         corpus can only ever score precision"
    );

    // A ratchet, not a target. Precision at `extracted` is fenced at 1.0
    // because that tier's contract admits no exceptions; recall is allowed to
    // be imperfect, but it is not allowed to *collapse* — a kernel that quietly
    // stops reporting real dead code passes every precision check ever written,
    // because claiming nothing is trivially precise.
    //
    // Measured, not chosen: setting `DEAD_CLUSTER_CAP` to 0 dropped this from
    // 4/4 to 2/4 and the reporting above printed the change without failing.
    // Raise the floor when recall improves; a drop below it is a regression to
    // explain, not a number to edit.
    const RECALL_FLOOR: f64 = 1.0;
    let recall = total_found as f64 / total_dead as f64;
    assert!(
        recall >= RECALL_FLOOR,
        "recall fell to {recall:.3} (found {total_found} of {total_dead} labelled \
         dead symbols), below the {RECALL_FLOOR:.3} floor. A dead-code tool that \
         stops finding dead code still scores perfect precision."
    );
}
