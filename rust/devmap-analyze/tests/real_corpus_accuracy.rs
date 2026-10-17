//! S1 — the corpus half of the accuracy harness: a committed record of how
//! DevMap does on the owner's real repositories, run locally and never in CI.
//!
//! The corpora are private and CI has no scheduled job, so the measurement is a
//! local `#[ignore]`d run that writes `testdata/accuracy/record.json`:
//!
//! ```text
//! DEVMAP_CORPUS_SCHOLARLM=~/src/scholarlm \
//! DEVMAP_CORPUS_LAPPI_DECISION=~/src/Lappi-decision \
//!   cargo test --release -p devmap-analyze --test real_corpus_accuracy \
//!   -- --ignored --nocapture
//! ```
//!
//! * **Pinned.** `corpora.json` names the one commit each corpus is measured at.
//!   A checkout at any other commit, or with local changes, is refused: a
//!   number measured on a moving target is not a record.
//! * **Label-free numbers always.** Edge counts by resolution kind, HIGH edges,
//!   UniqueGlobal / AmbiguousGlobal counts, unresolved rows by class and the
//!   resolution rate. These are written under `measurements`.
//! * **Labelled numbers when labels exist.** Precision, recall by construct,
//!   honest-miss and dead false positives need a human (or an oracle) to say what
//!   is true. Those labels name private paths and symbols, so they live *beside
//!   the corpus* (`<checkout>/.devmap-accuracy/labels.json`, or
//!   `DEVMAP_CORPUS_<NAME>_LABELS`), never in this repository. A corpus with
//!   labels gets a `corpora` row, the shape `devmap-cli/tests/
//!   dead_tier_eligibility.rs` reads; one without gets none, because a missing
//!   number must not look like a passing one.
//! * **Gated.** [`evaluate_gates`]: confident precision >= 0.95, honest-miss >=
//!   98%, 0.9-tier dead false positives <= 5%, no regression against the
//!   previous row (precision -1 pt, recall -2 pt) and the recall floors in
//!   `corpora.json`. Precision, honest-miss and dead FP are absolute: open
//!   whenever missed, even on a first measurement. The recall floors are a
//!   ratchet: one that held must keep holding, one that did not may only not
//!   get worse, so each language card tightens it. On top of that,
//!   [`claim_gates`] holds the cards' claims absolutely: recall at the card
//!   target (`recall_targets_percent`), UniqueGlobal precision >= 0.95 and zero
//!   AmbiguousGlobal edges. A gate whose number is missing - no labels, no
//!   measurement - is unmeasured and fails; a corpus without labels is gated,
//!   not skipped.
//! * **Fresh.** The runner refuses a DevMap that lacks any commit in
//!   `corpora.json.record_must_include` (S2 and the language lanes). Replaced
//!   rows move to `record.superseded`; `record.open_gates` lists every gate the
//!   committed record does not meet, and CI checks that list against the record.
//!
//! Run it before any DevMap release and after a change to `treesitter.rs`,
//! `langcalls/`, `langdecl/`, `resolver.rs` or `liveness.rs` that touches the
//! language. `the_scorer_*` below run in CI over a synthetic corpus so the
//! scorer itself is never the unmeasured part.

mod common;

use common::Built;
use devmap_analyze::dead_tier::in_confident_branch;
use devmap_analyze::record_identity::{is_evidence_category, is_identity_hash};
use devmap_analyze::{DeadSymbolReport, COVERAGE_LOSS_REASON};
use devmap_extract::model::{EdgeKind, SymbolKind};
use devmap_resolve::UNATTRIBUTED_LABELS;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const CONFIDENT_PRECISION_FLOOR: f64 = 0.95;
const HONEST_MISS_FLOOR: f64 = 0.98;
const DEAD_FP_CEILING: f64 = 0.05;
/// How many ids (or malformed-label reasons) a `dead_unmeasured` block lists.
const UNMEASURED_SAMPLE_CAP: usize = 5;
const PRECISION_TOLERANCE: f64 = 0.01;
const RECALL_TOLERANCE: f64 = 0.02;
const HIGH_MILLIS: i64 = 900;
/// S2's contract: a UniqueGlobal edge (a bare-name bind with no other
/// evidence) is right at least this often.
const UNIQUE_GLOBAL_PRECISION_FLOOR: f64 = 0.95;

/// The recall keys `corpora.json` must carry, floor and target alike.
const RECALL_KEYS: [&str; 7] = [
    "go",
    "python",
    "typescript",
    "rust_free_self",
    "rust_non_self",
    "swift",
    "kotlin",
];

/// The lowest each recall *floor* (the E3 baseline: today's measurement minus
/// two points) may ever be. A language card raises its floor in its own
/// change; nothing lowers one. Lowering `corpora.json` below this fails
/// `no_recall_floor_or_card_target_is_ever_lowered`.
const MIN_RECALL_FLOORS_PERCENT: [(&str, f64); 7] = [
    ("go", 88.0),
    ("python", 78.0),
    ("typescript", 67.0),
    ("rust_free_self", 91.0),
    ("rust_non_self", 46.0),
    ("swift", 80.0),
    ("kotlin", 60.0),
];

/// The recall each language card claims (audit 2026-10-13). These are gated
/// absolutely: below target, or unmeasured, fails.
const MIN_RECALL_TARGETS_PERCENT: [(&str, f64); 7] = [
    ("go", 95.0),
    ("python", 88.0),
    ("typescript", 85.0),
    ("rust_free_self", 95.0),
    ("rust_non_self", 75.0),
    ("swift", 90.0),
    ("kotlin", 85.0),
];

fn accuracy_root() -> PathBuf {
    common::accuracy_root()
}

fn record_path() -> PathBuf {
    accuracy_root().join("record.json")
}

// ------------------------------------------------------------------- pins

#[derive(Debug, Clone)]
struct Pin {
    name: String,
    commit: String,
    languages: Vec<String>,
}

/// What the record is gated on, from `corpora.json`.
#[derive(Debug, Clone)]
struct Gates {
    /// E3 baseline floors, held as a ratchet against the previous record.
    floors: BTreeMap<String, f64>,
    /// The language cards' recall targets, gated absolutely.
    targets: BTreeMap<String, f64>,
    /// DevMap commits a measurement must be taken on top of (S2 and every
    /// language lane). A row measured before them describes a DevMap that no
    /// longer exists.
    must_include: Vec<String>,
}

fn rates(value: &Value, key: &str, path: &Path) -> BTreeMap<String, f64> {
    let table = value[key]
        .as_object()
        .unwrap_or_else(|| panic!("{}: no {key} object", path.display()));
    let out: BTreeMap<String, f64> = table
        .iter()
        .map(|(k, v)| {
            let pct = v
                .as_f64()
                .unwrap_or_else(|| panic!("{}: {key}.{k} is not a number", path.display()));
            (k.clone(), pct / 100.0)
        })
        .collect();
    for k in RECALL_KEYS {
        assert!(
            out.contains_key(k),
            "{}: {key} has no {k}; an ungated language is not a passing one",
            path.display()
        );
    }
    out
}

fn load_pins() -> (Vec<Pin>, Gates) {
    let path = accuracy_root().join("corpora.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let pins = value["corpora"]
        .as_array()
        .expect("corpora is an array")
        .iter()
        .map(|c| Pin {
            name: c["name"].as_str().expect("name").to_string(),
            commit: c["commit"].as_str().expect("commit").to_string(),
            languages: c["languages"]
                .as_array()
                .expect("languages")
                .iter()
                .map(|l| l.as_str().expect("language").to_string())
                .collect(),
        })
        .collect();
    let must_include: Vec<String> = value["record_must_include"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: no record_must_include array", path.display()))
        .iter()
        .map(|c| {
            c["commit"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "{}: record_must_include row {c} has no commit",
                        path.display()
                    )
                })
                .to_string()
        })
        .collect();
    assert!(
        !must_include.is_empty(),
        "{}: record_must_include is empty, so a stale record cannot be told from a fresh one",
        path.display()
    );
    let gates = Gates {
        floors: rates(&value, "recall_floors_percent", &path),
        targets: rates(&value, "recall_targets_percent", &path),
        must_include,
    };
    (pins, gates)
}

fn env_name(corpus: &str) -> String {
    let upper: String = corpus
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("DEVMAP_CORPUS_{upper}")
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Refuses any checkout that is not the pinned commit, clean.
fn verify_pin(dir: &Path, pin: &Pin) -> Result<(), String> {
    let head = git(dir, &["rev-parse", "HEAD"])?;
    if head != pin.commit {
        return Err(format!(
            "{} is at {head}, not the pinned {}: a record measured elsewhere is not the record",
            pin.name, pin.commit
        ));
    }
    let dirty = git(dir, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.is_empty() {
        return Err(format!(
            "{} has uncommitted changes to tracked files; refusing to measure",
            pin.name
        ));
    }
    Ok(())
}

// ----------------------------------------------------------------- labels

#[derive(Debug, Default)]
struct Labels {
    /// A DevMap edge a reader judged: `caller -> callee`, right or wrong.
    edges: Vec<EdgeLabel>,
    /// A call site whose true callee a reader determined.
    recall: Vec<RecallLabel>,
    /// A dead-code row a reader classified.
    dead: Vec<DeadLabel>,
    /// What a type checker bound, per language: judges every confident edge
    /// from a caller it read, with no per-edge label.
    oracle: Vec<OracleLabel>,
}

/// A type checker's account of one language: every use of an in-repo function
/// or method it bound, from every caller it read. Unlike [`EdgeLabel`] it is a
/// function of the corpus alone, so it does not go stale when DevMap's edges
/// move: an edge is judged right when the oracle bound the same pair, wrong
/// when it did not.
#[derive(Debug)]
struct OracleLabel {
    language: String,
    /// Every caller id the oracle read. An edge from any other caller (a
    /// build-tagged file the oracle did not load) is not judged at all.
    covered_callers: BTreeSet<String>,
    /// `(caller, callee)` for every use the oracle bound; an interface method
    /// use also binds the caller to each in-repo implementation.
    uses: BTreeSet<(String, String)>,
}

#[derive(Debug)]
struct EdgeLabel {
    language: String,
    caller: String,
    callee: String,
    correct: bool,
}

#[derive(Debug)]
struct RecallLabel {
    language: String,
    /// Reporting class: a construct name, or `free_self` / `non_self` for Rust.
    construct: String,
    caller: String,
    /// The name written at the call site.
    name: String,
    /// The true in-repo callee symbol id; `a|b` when an edge to any one of
    /// them is the call (an interface method or one of its implementations).
    truth: String,
}

#[derive(Debug)]
struct DeadLabel {
    language: String,
    id: String,
    truly_dead: bool,
    /// A reader could not settle it: listed in the record, scored as neither
    /// right nor wrong.
    unsure: bool,
    /// `h:` plus twelve hex digits of the SHA-256 of `id`, computed beside the
    /// corpus. Required for every corpus but DevCouncil's own.
    symbol_hash: Option<String>,
    /// One of `record_identity::EVIDENCE_CATEGORIES`. Required likewise.
    category: Option<String>,
    /// The labeller's prose; only DevCouncil's own (public) rows carry it on.
    evidence: Option<String>,
}

/// The one corpus whose rows stay readable in the committed record.
const OWN_CORPUS: &str = "DevCouncil";

/// The `labelled_rows[]` entry for a dead label that matched a row at `tier`.
///
/// A private corpus's row carries `symbol_hash` and `category` from the labels
/// and nothing else of the labeller's: a label missing either, or carrying one
/// that fails the shared shape check, is an error and never falls back to the
/// raw id or prose. The error names the corpus, language and label index, not
/// the offending string.
fn labelled_row(
    corpus: &str,
    lang: &str,
    index: usize,
    l: &DeadLabel,
    tier: &str,
) -> Result<Value, String> {
    let at = format!("{corpus} {lang} dead label {index}");
    let verdict = if l.unsure {
        "unsure"
    } else if l.truly_dead {
        "dead_true"
    } else {
        "dead_false"
    };
    let (symbol, evidence) = if corpus == OWN_CORPUS {
        let evidence = l
            .evidence
            .clone()
            .ok_or_else(|| format!("{at}: `evidence` is missing"))?;
        (l.id.clone(), evidence)
    } else {
        let symbol = l
            .symbol_hash
            .as_deref()
            .filter(|h| is_identity_hash(h))
            .ok_or_else(|| {
                format!(
                    "{at}: `symbol_hash` is missing or is not `h:` and twelve lowercase hex digits"
                )
            })?;
        let evidence = l
            .category
            .as_deref()
            .filter(|c| is_evidence_category(c))
            .ok_or_else(|| {
                format!("{at}: `category` is missing or is not an evidence category in the README")
            })?;
        (symbol.to_string(), evidence.to_string())
    };
    Ok(json!({ "symbol": symbol, "evidence": evidence, "verdict": verdict, "published": tier }))
}

fn parse_labels(raw: &str) -> Result<Labels, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("labels are not JSON: {e}"))?;
    let text = |o: &Value, k: &str| -> Result<String, String> {
        o[k].as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("label {o} has no string {k:?}"))
    };
    let mut labels = Labels::default();
    for o in v["edges"].as_array().into_iter().flatten() {
        labels.edges.push(EdgeLabel {
            language: text(o, "language")?,
            caller: text(o, "caller")?,
            callee: text(o, "callee")?,
            correct: o["correct"]
                .as_bool()
                .ok_or_else(|| format!("label {o} has no bool correct"))?,
        });
    }
    for o in v["recall"].as_array().into_iter().flatten() {
        labels.recall.push(RecallLabel {
            language: text(o, "language")?,
            construct: text(o, "construct")?,
            caller: text(o, "caller")?,
            name: text(o, "name")?,
            truth: text(o, "truth")?,
        });
    }
    for o in v["dead"].as_array().into_iter().flatten() {
        labels.dead.push(DeadLabel {
            language: text(o, "language")?,
            id: text(o, "id")?,
            truly_dead: o["truly_dead"]
                .as_bool()
                .ok_or_else(|| format!("label {o} has no bool truly_dead"))?,
            unsure: o["unsure"].as_bool().unwrap_or(false),
            symbol_hash: o["symbol_hash"].as_str().map(str::to_string),
            category: o["category"].as_str().map(str::to_string),
            evidence: o["evidence"].as_str().map(str::to_string),
        });
    }
    let strings = |o: &Value, k: &str| -> Result<Vec<String>, String> {
        o[k].as_array()
            .ok_or_else(|| format!("oracle entry has no array {k:?}"))?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("oracle {k:?} holds a non-string: {v}"))
            })
            .collect()
    };
    for o in v["oracle"].as_array().into_iter().flatten() {
        let language = text(o, "language")?;
        let covered_callers: BTreeSet<String> =
            strings(o, "covered_callers")?.into_iter().collect();
        let mut uses = BTreeSet::new();
        for u in o["uses"]
            .as_array()
            .ok_or_else(|| format!("oracle entry for {language} has no array \"uses\""))?
        {
            let caller = text(u, "caller")?;
            if !covered_callers.contains(&caller) {
                return Err(format!(
                    "oracle use {u} is from a caller not in covered_callers"
                ));
            }
            for callee in std::iter::once(text(u, "callee")?).chain(strings(u, "impls")?) {
                uses.insert((caller.clone(), callee));
            }
        }
        labels.oracle.push(OracleLabel {
            language,
            covered_callers,
            uses,
        });
    }
    Ok(labels)
}

fn load_labels(corpus_dir: &Path, name: &str) -> Result<Option<Labels>, String> {
    let path = std::env::var_os(format!("{}_LABELS", env_name(name)))
        .map(PathBuf::from)
        .unwrap_or_else(|| corpus_dir.join(".devmap-accuracy/labels.json"));
    match std::fs::read_to_string(&path) {
        Ok(raw) => parse_labels(&raw)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

// ---------------------------------------------------------------- scoring

fn language_of(path: &str) -> String {
    match devmap_extract::languages::detect_language(Path::new(path)) {
        "tsx" | "javascript" | "jsx" => "typescript".to_string(),
        other => other.to_string(),
    }
}

fn is_use(kind: EdgeKind) -> bool {
    !matches!(
        kind,
        EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
    )
}

#[derive(Default)]
struct Counts {
    correct: usize,
    total: usize,
}

impl Counts {
    fn record(&mut self, ok: bool) {
        self.total += 1;
        self.correct += usize::from(ok);
    }
    fn rate(&self) -> Option<f64> {
        (self.total > 0).then(|| self.correct as f64 / self.total as f64)
    }
    fn json(&self) -> Value {
        json!({ "correct": self.correct, "total": self.total, "rate": self.rate() })
    }
}

/// Last `.`/`:`-separated segment of a symbol or call name.
fn last_segment(name: &str) -> &str {
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

/// Recall sites whose truth edge is absent, split by what DevMap did instead.
/// `recorded`: an unattributed ledger row names the call. `silent`: no ledger
/// row and no edge at all. `wrongly_bound`: a HIGH edge for that call name to a
/// non-truth target - precision's defect, reported here but kept out of the
/// rate, which is `recorded / (recorded + silent)`.
#[derive(Default, Debug, PartialEq, Eq)]
struct HonestMiss {
    recorded: usize,
    silent: usize,
    wrongly_bound: usize,
}

impl HonestMiss {
    fn rate(&self) -> Option<f64> {
        let denominator = self.recorded + self.silent;
        (denominator > 0).then(|| self.recorded as f64 / denominator as f64)
    }
    /// `absent` keeps its meaning (every site whose truth edge is missing) so
    /// rows committed before the split stay comparable.
    fn json(&self) -> Value {
        json!({
            "recorded": self.recorded,
            "silent": self.silent,
            "wrongly_bound": self.wrongly_bound,
            "absent": self.recorded + self.silent + self.wrongly_bound,
            "rate": self.rate(),
        })
    }
}

fn tier_label(r: &DeadSymbolReport) -> &'static str {
    if in_confident_branch(f64::from(r.confidence), r.exemption_reason.as_deref()) {
        "0.9"
    } else {
        "0.4"
    }
}

/// Label-free structure per language.
fn structure(built: &Built, languages: &[String]) -> BTreeMap<String, Value> {
    let mut by_kind: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut high: BTreeMap<String, usize> = BTreeMap::new();
    for edge in built
        .resolution
        .edges
        .iter()
        .filter(|e| is_use(e.edge_kind))
    {
        let lang = language_of(&edge.source_file);
        let kind = edge
            .resolution
            .as_ref()
            .map_or("none", |r| r.kind().label())
            .to_string();
        *by_kind
            .entry(lang.clone())
            .or_default()
            .entry(kind)
            .or_default() += 1;
        if edge.confidence.to_millis() >= HIGH_MILLIS {
            *high.entry(lang).or_default() += 1;
        }
    }
    let mut unresolved: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    for u in &built.resolution.unresolved {
        *unresolved
            .entry(language_of(&u.source_file))
            .or_default()
            .entry(u.class.label())
            .or_default() += 1;
    }
    let mut dead: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    for r in built.analysis.dead_symbols.iter().filter(|r| !r.is_exempt) {
        *dead
            .entry(language_of(&r.file_path))
            .or_default()
            .entry(tier_label(r))
            .or_default() += 1;
    }
    let rate = &built.analysis.resolution_rate.by_language;

    languages
        .iter()
        .map(|lang| {
            let kinds = by_kind.get(lang).cloned().unwrap_or_default();
            let unres = unresolved.get(lang).cloned().unwrap_or_default();
            let attributed: usize = kinds.values().sum();
            let unattributed: usize = unres
                .iter()
                .filter(|(label, _)| UNATTRIBUTED_LABELS.contains(label))
                .map(|(_, n)| n)
                .sum();
            (
                lang.clone(),
                json!({
                    "use_edges": attributed,
                    "use_edges_by_resolution": kinds,
                    "high_confidence_edges": high.get(lang).copied().unwrap_or(0),
                    "ambiguous_global_edges": kinds.get("AmbiguousGlobal").copied().unwrap_or(0),
                    "unresolved_rows_by_class": unres,
                    "unattributed_rows": unattributed,
                    "net_resolution_permille": rate.get(lang).and_then(|r| r.net_permille),
                    "dead_rows_by_tier": dead.get(lang).cloned().unwrap_or_default(),
                }),
            )
        })
        .collect()
}

/// What `labels` say about this build, per language. Fails loudly on a label
/// that no longer matches the graph: a stale label scored as a miss would
/// quietly turn a moved symbol into a regression.
fn score_labels(
    corpus: &str,
    built: &Built,
    labels: &Labels,
    languages: &[String],
) -> Result<BTreeMap<String, Value>, String> {
    let mut edge_conf: BTreeMap<(&str, &str), (i64, &'static str)> = BTreeMap::new();
    let mut any_edge: BTreeSet<(&str, &str)> = BTreeSet::new();
    // `(caller, last segment of the target's name)` for every HIGH call edge:
    // what a recall site was bound to when its truth edge is absent. Read from
    // the build's edges, never from labels.
    let mut high_bound: BTreeSet<(&str, &str)> = BTreeSet::new();
    for e in built
        .resolution
        .edges
        .iter()
        .filter(|e| is_use(e.edge_kind))
    {
        any_edge.insert((e.source_symbol.as_str(), e.target_symbol.as_str()));
        if matches!(e.edge_kind, EdgeKind::Calls | EdgeKind::References)
            && e.confidence.to_millis() >= HIGH_MILLIS
        {
            high_bound.insert((e.source_symbol.as_str(), last_segment(&e.target_symbol)));
        }
        let kind = e.resolution.as_ref().map_or("none", |r| r.kind().label());
        let entry = edge_conf
            .entry((e.source_symbol.as_str(), e.target_symbol.as_str()))
            .or_insert((e.confidence.to_millis(), kind));
        if e.confidence.to_millis() > entry.0 {
            *entry = (e.confidence.to_millis(), kind);
        }
    }
    let mut ledger: BTreeMap<&str, Vec<(&str, &'static str)>> = BTreeMap::new();
    for u in &built.resolution.unresolved {
        ledger
            .entry(u.source_symbol.as_str())
            .or_default()
            .push((u.callee_name.as_str(), u.class.label()));
    }
    // Every dead row by id: its tier, and the language of its file.
    let mut dead_tier: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    for r in built.analysis.dead_symbols.iter().filter(|r| !r.is_exempt) {
        dead_tier.insert(
            format!("{}::{}", r.file_path, r.symbol_name),
            (tier_label(r), language_of(&r.file_path)),
        );
    }
    // What an oracle judges: the strongest Calls/References edge per pair from
    // a caller in `lang` to a function or method. Extends/Implements and the
    // structural kinds are not uses a call-site oracle can see.
    let callable: BTreeSet<&str> = built
        .extractions
        .iter()
        .flat_map(|x| &x.symbols)
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .map(|s| s.qualified_name.as_str())
        .collect();
    let mut judgeable: BTreeMap<(&str, &str), (i64, &'static str, String)> = BTreeMap::new();
    for e in built.resolution.edges.iter().filter(|e| {
        matches!(e.edge_kind, EdgeKind::Calls | EdgeKind::References)
            && callable.contains(e.target_symbol.as_str())
    }) {
        let kind = e.resolution.as_ref().map_or("none", |r| r.kind().label());
        let millis = e.confidence.to_millis();
        let entry = judgeable
            .entry((e.source_symbol.as_str(), e.target_symbol.as_str()))
            .or_insert((millis, kind, language_of(&e.source_file)));
        if millis > entry.0 {
            entry.0 = millis;
            entry.1 = kind;
        }
    }

    let mut out = BTreeMap::new();
    for lang in languages {
        let has_labels = labels.edges.iter().any(|l| &l.language == lang)
            || labels.recall.iter().any(|l| &l.language == lang)
            || labels.dead.iter().any(|l| &l.language == lang)
            || labels.oracle.iter().any(|l| &l.language == lang);
        // Labels that say nothing about a pinned language leave it
        // unmeasured: no row, so `claim_gates` reports it as such. An empty
        // row would read as measured.
        if !has_labels {
            continue;
        }
        let mut confident = Counts::default();
        let mut unique = Counts::default();
        let hand_labelled: BTreeSet<(&str, &str)> = labels
            .edges
            .iter()
            .filter(|l| &l.language == lang)
            .map(|l| (l.caller.as_str(), l.callee.as_str()))
            .collect();
        for oracle in labels.oracle.iter().filter(|o| &o.language == lang) {
            for ((caller, callee), (millis, kind, from)) in &judgeable {
                if from != lang
                    || hand_labelled.contains(&(*caller, *callee))
                    || !oracle.covered_callers.contains(*caller)
                {
                    continue;
                }
                let correct = oracle
                    .uses
                    .contains(&((*caller).to_string(), (*callee).to_string()));
                if *millis >= HIGH_MILLIS {
                    confident.record(correct);
                }
                if *kind == "UniqueGlobal" {
                    unique.record(correct);
                }
            }
        }
        for l in labels.edges.iter().filter(|l| &l.language == lang) {
            let Some((millis, kind)) = edge_conf.get(&(l.caller.as_str(), l.callee.as_str()))
            else {
                return Err(format!(
                    "edge label {} -> {} ({lang}) matches no edge in this build; regenerate the labels",
                    l.caller, l.callee
                ));
            };
            if *millis >= HIGH_MILLIS {
                confident.record(l.correct);
            }
            if *kind == "UniqueGlobal" {
                unique.record(l.correct);
            }
        }

        let mut by_construct: BTreeMap<&str, Counts> = BTreeMap::new();
        let mut honest = HonestMiss::default();
        for l in labels.recall.iter().filter(|l| &l.language == lang) {
            let present = l
                .truth
                .split('|')
                .any(|truth| any_edge.contains(&(l.caller.as_str(), truth)));
            by_construct
                .entry(l.construct.as_str())
                .or_default()
                .record(present);
            if !present {
                let recorded = ledger.get(l.caller.as_str()).is_some_and(|rows| {
                    rows.iter().any(|(name, class)| {
                        UNATTRIBUTED_LABELS.contains(class)
                            && last_segment(name) == last_segment(&l.name)
                    })
                });
                // A HIGH edge for this call name to a non-truth target is a
                // wrong binding, which precision scores; it is neither an
                // honest nor a silent miss.
                if high_bound.contains(&(l.caller.as_str(), last_segment(&l.name))) {
                    honest.wrongly_bound += 1;
                } else if recorded {
                    honest.recorded += 1;
                } else {
                    honest.silent += 1;
                }
            }
        }

        // The confident tier is always written, so a row has the shape
        // `dead_tier_eligibility.rs` reads. A confident-branch row nobody
        // labelled, and a label that cannot be written safely, are not scored
        // and are never a pass: both are counted and reported as unmeasured
        // (`dead_unmeasured`), the run goes on, and `dead_fp_rate` then reads
        // the language's dead gate as unmeasured instead of a rate over a
        // subset.
        let mut dead_fp: BTreeMap<&str, (usize, usize)> = BTreeMap::from([("0.9", (0, 0))]);
        let mut labelled_rows = Vec::new();
        let mut unsure = 0;
        let mut malformed: Vec<String> = Vec::new();
        let mut malformed_ids: BTreeSet<&str> = BTreeSet::new();
        let mut well_formed_ids: BTreeSet<&str> = BTreeSet::new();
        for (index, l) in labels
            .dead
            .iter()
            .filter(|l| &l.language == lang)
            .enumerate()
        {
            // Every label is checked, matched or not, so one that could not be
            // written safely is reported rather than waiting for the build to
            // reach it.
            let tier = dead_tier.get(&l.id).map(|(tier, _)| *tier);
            match labelled_row(corpus, lang, index, l, tier.unwrap_or("none")) {
                Err(why) => {
                    malformed.push(why);
                    malformed_ids.insert(l.id.as_str());
                }
                Ok(row) => {
                    well_formed_ids.insert(l.id.as_str());
                    if let Some(tier) = tier {
                        labelled_rows.push(row);
                        if l.unsure {
                            unsure += 1;
                        } else {
                            let entry = dead_fp.entry(tier).or_default();
                            entry.1 += 1;
                            entry.0 += usize::from(!l.truly_dead);
                        }
                    }
                }
            }
        }
        // A row whose label is malformed is counted once, as malformed.
        let unlabelled: Vec<&str> = dead_tier
            .iter()
            .filter(|(id, (tier, of))| {
                *tier == "0.9"
                    && of == lang
                    && !well_formed_ids.contains(id.as_str())
                    && !malformed_ids.contains(id.as_str())
            })
            .map(|(id, _)| id.as_str())
            .collect();
        // A private corpus's raw ids never reach the committed record; its
        // sample goes to the run's output instead.
        let sample: Vec<&str> = unlabelled
            .iter()
            .take(UNMEASURED_SAMPLE_CAP)
            .copied()
            .collect();
        if !unlabelled.is_empty() {
            eprintln!(
                "{corpus} {lang}: {} confident-branch dead row(s) have no dead label and are unmeasured (first {}): {}",
                unlabelled.len(),
                sample.len(),
                sample.join(", ")
            );
        }
        let malformed_sample: Vec<&String> = malformed.iter().take(UNMEASURED_SAMPLE_CAP).collect();
        let dead_unmeasured = json!({
            "unlabelled": unlabelled.len(),
            "unlabelled_sample": if corpus == OWN_CORPUS { json!(sample) } else { json!([]) },
            "malformed": malformed.len(),
            "malformed_sample": malformed_sample,
            "sample_cap": UNMEASURED_SAMPLE_CAP,
        });

        let recall_total: usize = by_construct.values().map(|c| c.total).sum();
        let recall_present: usize = by_construct.values().map(|c| c.correct).sum();
        let by_construct_json: Map<String, Value> = by_construct
            .iter()
            .map(|(k, c)| ((*k).to_string(), c.json()))
            .collect();
        out.insert(
            lang.clone(),
            json!({
                "confident_edge_precision": confident.json(),
                "unique_global_precision": unique.json(),
                "recall": {
                    "present": recall_present,
                    "total": recall_total,
                    "rate": (recall_total > 0).then(|| recall_present as f64 / recall_total as f64),
                    "by_construct": by_construct_json,
                },
                "honest_miss": honest.json(),
                "dead_fp_by_tier": dead_fp
                    .iter()
                    .map(|(tier, (fp, rows))| ((*tier).to_string(), json!({ "false_positive": fp, "rows": rows })))
                    .collect::<Map<String, Value>>(),
                "labelled_rows": labelled_rows,
                "dead_unmeasured": dead_unmeasured,
                "unsure": unsure,
            }),
        );
    }
    Ok(out)
}

// ------------------------------------------------------------------ gates

fn rate_of(row: &Value, path: &[&str]) -> Option<f64> {
    path.iter()
        .try_fold(row, |v, k| v.get(*k))
        .and_then(Value::as_f64)
}

/// Rows the dead gate could not score: confident-branch rows with no label plus
/// labels that were malformed.
fn dead_unmeasured_count(row: &Value) -> u64 {
    let n = |k: &str| row["dead_unmeasured"][k].as_u64().unwrap_or(0);
    n("unlabelled") + n("malformed")
}

/// What a row says about the 0.9-tier dead false-positive gate.
#[derive(Debug, PartialEq)]
enum DeadFp {
    /// Every confident row was labelled and there is at least one.
    Measured(f64),
    /// The corpus was labelled and scored, and no row reached the confident
    /// tier: there is nothing to be wrong about, which is not evidence of
    /// accuracy. It does not pass. Whether an empty tier may pass is an open
    /// owner decision; the default is strict.
    EmptyTier,
    /// No labels, an unlabelled or malformed row, or a row that cannot be read.
    Unmeasured,
}

fn dead_fp_state(row: &Value) -> DeadFp {
    if dead_unmeasured_count(row) > 0 {
        return DeadFp::Unmeasured;
    }
    let tier = &row["dead_fp_by_tier"]["0.9"];
    let (Some(fp), Some(rows)) = (tier["false_positive"].as_f64(), tier["rows"].as_f64()) else {
        return DeadFp::Unmeasured;
    };
    if rows > 0.0 {
        DeadFp::Measured(fp / rows)
    } else {
        DeadFp::EmptyTier
    }
}

/// The measured rate, `None` for both non-measured states.
fn dead_fp_rate(row: &Value) -> Option<f64> {
    match dead_fp_state(row) {
        DeadFp::Measured(rate) => Some(rate),
        DeadFp::EmptyTier | DeadFp::Unmeasured => None,
    }
}

/// The recall to hold against a `table` of floors or targets: Rust is gated on
/// its two classes separately, every other language on the overall rate.
/// `Err` when the table has no entry for the language: an ungated language
/// must be reported, never skipped.
fn recall_checks(
    lang: &str,
    row: &Value,
    table: &BTreeMap<String, f64>,
) -> Result<Vec<(String, Option<f64>, f64)>, String> {
    let keyed: Vec<(String, String, Option<f64>)> = if lang == "rust" {
        ["free_self", "non_self"]
            .iter()
            .map(|class| {
                (
                    format!("rust_{class}"),
                    format!("recall[{class}]"),
                    row["recall"]["by_construct"][*class]["rate"].as_f64(),
                )
            })
            .collect()
    } else {
        vec![(
            lang.to_string(),
            "recall".to_string(),
            rate_of(row, &["recall", "rate"]),
        )]
    };
    keyed
        .into_iter()
        .map(|(key, name, rate)| {
            table
                .get(&key)
                .map(|floor| (name, rate, *floor))
                .ok_or_else(|| {
                    format!("{lang}: corpora.json sets no recall for {key}, so it is ungated")
                })
        })
        .collect()
}

/// Every gate violation for one corpus language: [`regression_gates`] against
/// the previous row, then [`claim_gates`] against the cards' targets.
fn evaluate_gates(
    lang: &str,
    previous: Option<&Value>,
    current: &Value,
    measurement: Option<&Value>,
    gates: &Gates,
) -> Vec<String> {
    // Numbers carried from a row this run could not re-check are not evidence:
    // the unverifiable row fails by itself and its stale rates are not read.
    if let Some(reason) = current["unverifiable"].as_str() {
        return vec![unverifiable_gate(lang, reason)];
    }
    let mut out = regression_gates(lang, previous, current, &gates.floors);
    out.extend(claim_gates(lang, current, measurement, gates));
    out
}

/// The absolute claims the language cards and S2 make, which hold whatever the
/// previous record said: recall at or above the card's target, UniqueGlobal
/// precision >= 0.95, and no AmbiguousGlobal edge at all. Each is unmeasured -
/// and so failing - when the number it needs is missing: `current` is
/// `Value::Null` for a corpus without labels, `measurement` is `None` for a
/// language the label-free pass never measured.
fn claim_gates(
    lang: &str,
    current: &Value,
    measurement: Option<&Value>,
    gates: &Gates,
) -> Vec<String> {
    let mut out = Vec::new();
    match recall_checks(lang, current, &gates.targets) {
        Err(e) => out.push(e),
        Ok(checks) => {
            for (name, now, target) in checks {
                match now {
                    None => out.push(format!(
                        "{lang}: {name} card target {target:.3} is unmeasured (no labels), so it cannot pass"
                    )),
                    Some(v) if v < target => out.push(format!(
                        "{lang}: {name} {v:.3} is below the card target {target:.3}"
                    )),
                    Some(_) => {}
                }
            }
        }
    }

    let unique_edges = measurement
        .and_then(|m| m["use_edges_by_resolution"].as_object())
        .map(|kinds| {
            kinds
                .get("UniqueGlobal")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        });
    match (
        rate_of(current, &["unique_global_precision", "rate"]),
        unique_edges,
    ) {
        // No UniqueGlobal edge exists to be wrong: nothing to judge.
        (None, Some(0)) => {}
        (None, _) => out.push(format!(
            "{lang}: UniqueGlobal precision is unmeasured (no labelled UniqueGlobal edge), so it cannot pass"
        )),
        (Some(v), _) if v < UNIQUE_GLOBAL_PRECISION_FLOOR => out.push(format!(
            "{lang}: UniqueGlobal precision {v:.3} is below {UNIQUE_GLOBAL_PRECISION_FLOOR:.3}"
        )),
        (Some(_), _) => {}
    }

    match measurement.and_then(|m| m["ambiguous_global_edges"].as_u64()) {
        None => out.push(format!(
            "{lang}: AmbiguousGlobal edges are unmeasured (no label-free measurement), so the =0 gate cannot pass"
        )),
        Some(0) => {}
        Some(n) => out.push(format!(
            "{lang}: {n} AmbiguousGlobal edge(s); S2 makes AmbiguousGlobal ledger-only, so the gate is 0"
        )),
    }
    out
}

/// Gate violations for one language row against the previous one.
///
/// Two kinds of gate. An *absolute* one (confident precision, honest-miss,
/// 0.9-tier dead false positives) is open whenever the number misses its
/// threshold, with or without a previous row: a first measurement is not a
/// pass. A *ratchet* one (the recall floors, which are the E3 baseline and not
/// a claim) must keep holding once it held, and otherwise may only stay put
/// within tolerance; each language card is what raises it. Both also may not
/// regress against `previous`. A gate whose number is `None` - no labels, no
/// sample - is reported as unmeasured, never as passed.
fn regression_gates(
    lang: &str,
    previous: Option<&Value>,
    current: &Value,
    floors: &BTreeMap<String, f64>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |name: &str,
                     now: Option<f64>,
                     before: Option<f64>,
                     floor: f64,
                     higher_is_better: bool,
                     tolerance: f64,
                     absolute: bool| {
        let Some(now) = now else {
            out.push(format!(
                "{lang}: {name} is unmeasured (no labels), so it cannot pass"
            ));
            return;
        };
        let meets = |v: f64| {
            if higher_is_better {
                v >= floor
            } else {
                v <= floor
            }
        };
        let held = before.is_some_and(meets);
        if held && !meets(now) {
            out.push(format!(
                "{lang}: {name} {now:.3} no longer meets {floor:.3}, which the previous record did"
            ));
        } else if absolute && !meets(now) {
            out.push(format!(
                "{lang}: {name} {now:.3} does not meet the absolute floor {floor:.3}"
            ));
        }
        if let Some(before) = before {
            let regressed = if higher_is_better {
                now < before - tolerance
            } else {
                now > before + tolerance
            };
            if regressed {
                out.push(format!(
                    "{lang}: {name} regressed {before:.3} -> {now:.3} (tolerance {tolerance:.3})"
                ));
            }
        }
    };

    check(
        "confident edge precision",
        rate_of(current, &["confident_edge_precision", "rate"]),
        previous.and_then(|p| rate_of(p, &["confident_edge_precision", "rate"])),
        CONFIDENT_PRECISION_FLOOR,
        true,
        PRECISION_TOLERANCE,
        true,
    );
    check(
        "honest-miss rate",
        rate_of(current, &["honest_miss", "rate"]),
        previous.and_then(|p| rate_of(p, &["honest_miss", "rate"])),
        HONEST_MISS_FLOOR,
        true,
        RECALL_TOLERANCE,
        true,
    );
    let empty_tier = dead_fp_state(current) == DeadFp::EmptyTier;
    if !empty_tier {
        check(
            "0.9-tier dead false-positive rate",
            dead_fp_rate(current),
            previous.and_then(dead_fp_rate),
            DEAD_FP_CEILING,
            false,
            0.0,
            true,
        );
    }
    match recall_checks(lang, current, floors) {
        Err(e) => out.push(e),
        Ok(checks) => {
            for (name, now, floor) in checks {
                let before = previous.and_then(|p| {
                    recall_checks(lang, p, floors)
                        .ok()?
                        .into_iter()
                        .find(|(n, _, _)| *n == name)
                        .and_then(|(_, r, _)| r)
                });
                check(&name, now, before, floor, true, RECALL_TOLERANCE, false);
            }
        }
    }
    if empty_tier {
        out.push(format!(
            "{lang}: 0.9-tier dead false-positive rate: empty tier (0 rows), so it cannot pass; whether an empty tier may pass is an open owner decision, default strict"
        ));
    }
    out
}

/// Every gate the committed `record` does not meet, over every pinned corpus
/// and language, whether or not this run measured it. A row measured at a
/// DevMap commit that does not include every `gates.must_include` commit is
/// stale, and so is every number in it.
fn open_gates(record: &Value, pins: &[Pin], gates: &Gates) -> Vec<String> {
    let mut out = BTreeSet::new();
    let find = |list: &str, pin: &Pin| -> Option<Value> {
        record[list].as_array().and_then(|rows| {
            rows.iter()
                .find(|r| {
                    r["name"] == pin.name.as_str() && r["corpus_commit"] == pin.commit.as_str()
                })
                .cloned()
        })
    };
    for pin in pins {
        let measurement = find("measurements", pin);
        match &measurement {
            None => {
                out.insert(format!(
                    "{}: no label-free measurement at the pinned commit {}",
                    pin.name, pin.commit
                ));
            }
            Some(m) => {
                let included: BTreeSet<&str> = m["includes_commits"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let missing: Vec<&str> = gates
                    .must_include
                    .iter()
                    .map(String::as_str)
                    .filter(|c| !included.contains(c))
                    .collect();
                if !missing.is_empty() {
                    out.insert(format!(
                        "{}: stale - measured at DevMap {} ({}), not shown to include {} required commit(s) (S2 and the language lanes); re-run the corpus runner",
                        pin.name,
                        m["devmap_commit"].as_str().unwrap_or("unknown"),
                        m["date"].as_str().unwrap_or("undated"),
                        missing.len()
                    ));
                }
            }
        }
        let labelled = find("corpora", pin);
        let previous = record["superseded"].as_array().and_then(|rows| {
            rows.iter()
                .rev()
                .find(|r| r["list"] == "corpora" && r["entry"]["name"] == pin.name.as_str())
                .map(|r| r["entry"].clone())
        });
        for lang in &pin.languages {
            let current = labelled
                .as_ref()
                .map_or(Value::Null, |l| l["languages"][lang.as_str()].clone());
            let prev_row = previous
                .as_ref()
                .map(|p| &p["languages"][lang.as_str()])
                .filter(|v| !v.is_null());
            let lang_measurement = measurement
                .as_ref()
                .map(|m| &m["languages"][lang.as_str()])
                .filter(|v| !v.is_null());
            for f in evaluate_gates(lang, prev_row, &current, lang_measurement, gates) {
                out.insert(format!("{}: {f}", pin.name));
            }
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------- the run

fn devmap_commit() -> String {
    git(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &["rev-parse", "HEAD"],
    )
    .unwrap_or_else(|_| "unknown".into())
}

fn today() -> String {
    Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".into(), |s| s.trim().to_string())
}

fn load_record() -> Value {
    std::fs::read_to_string(record_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({ "schema": 1, "corpora": [], "measurements": [] }))
}

/// Refuses to measure with a DevMap that does not contain every required
/// commit: a record from before S2 or a language lane is not the record.
/// Returns the commits proven included, written into each measurement.
fn verify_devmap_includes(must_include: &[String]) -> Result<Vec<String>, String> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    for commit in must_include {
        let out = Command::new("git")
            .arg("-C")
            .arg(here)
            .args(["merge-base", "--is-ancestor", commit, "HEAD"])
            .output()
            .map_err(|e| format!("git merge-base: {e}"))?;
        match out.status.code() {
            Some(0) => {}
            Some(1) => {
                return Err(format!(
                    "this DevMap checkout does not include {commit}, which corpora.json requires of every record row"
                ))
            }
            _ => {
                return Err(format!(
                    "could not tell whether HEAD includes {commit}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        }
    }
    Ok(must_include.to_vec())
}

/// Measure one pinned corpus. Returns its `measurements` entry, and its
/// `corpora` entry when it has labels.
fn measure_corpus(
    dir: &Path,
    pin: &Pin,
    includes: &[String],
) -> Result<(Value, Option<Value>), String> {
    verify_pin(dir, pin)?;
    let built = common::build(dir);
    let structure = structure(&built, &pin.languages);
    let date = today();
    let commit = devmap_commit();
    let measurement = json!({
        "name": pin.name,
        "date": date,
        "devmap_commit": commit,
        "includes_commits": includes,
        "corpus_commit": pin.commit,
        "analysis_status": format!("{:?}", built.analysis.status),
        "dead_rows_total": built.analysis.dead_symbols.len(),
        "dead_rows_exempt": built.analysis.dead_symbols.iter().filter(|r| r.is_exempt).count(),
        "languages": structure,
    });
    let Some(labels) = load_labels(dir, &pin.name)? else {
        return Ok((measurement, None));
    };
    let scored = score_labels(&pin.name, &built, &labels, &pin.languages)?;
    let languages: Map<String, Value> = scored
        .into_iter()
        .map(|(lang, mut row)| {
            row["date"] = json!(date);
            row["devmap_commit"] = json!(commit);
            (lang, row)
        })
        .collect();
    Ok((
        measurement,
        Some(json!({ "name": pin.name, "corpus_commit": pin.commit, "languages": languages })),
    ))
}

/// The gate message for a committed labelled row this run could not re-check.
/// `open_gates` and the runner both prefix it with the corpus.
fn unverifiable_gate(lang: &str, reason: &str) -> String {
    format!("{lang}: committed labelled row is unverifiable: {reason}")
}

/// The `corpora` entry to write for `pin`, given the committed runner row and
/// this run's scored labels.
///
/// A language the record holds a labelled row for, and the run scored nothing
/// for (no labels file, or none for that language), is never dropped and never
/// left looking measured: its committed row is kept, stamped `unverifiable`
/// with the reason, and every gate then fails on it naming the corpus and
/// language ([`evaluate_gates`]). The next run that scores the language
/// replaces the row and clears the stamp.
fn carry_unverifiable(
    previous: Option<&Value>,
    labelled: Option<Value>,
    pin: &Pin,
) -> Option<Value> {
    let Some(previous) = previous else {
        return labelled;
    };
    let scored = |lang: &str| {
        labelled
            .as_ref()
            .is_some_and(|e| !e["languages"][lang].is_null())
    };
    let missing: Vec<&String> = pin
        .languages
        .iter()
        .filter(|lang| !previous["languages"][lang.as_str()].is_null() && !scored(lang))
        .collect();
    if missing.is_empty() {
        return labelled;
    }
    let reason = if labelled.is_none() {
        format!("no labels file was found for {}", pin.name)
    } else {
        "the labels hold nothing for this language".to_string()
    };
    let mut entry = labelled.unwrap_or_else(|| previous.clone());
    for lang in missing {
        let mut row = previous["languages"][lang.as_str()].clone();
        // A row already stamped keeps the reason it was first stamped with.
        if row["unverifiable"].is_null() {
            row["unverifiable"] = json!(reason);
        }
        entry["languages"][lang.as_str()] = row;
    }
    Some(entry)
}

/// A row this runner wrote for corpus `name`. Runner rows carry
/// `corpus_commit`; the hand-labelled dead record that shares `corpora` (and
/// the corpus name) carries `commit` instead, and is never one.
fn is_runner_row(entry: &Value, name: &str) -> bool {
    entry["name"] == name && entry.get("corpus_commit").is_some()
}

/// The runner row for corpus `name` in `record[list]`, if there is one.
fn runner_row<'a>(record: &'a Value, list: &str, name: &str) -> Option<&'a Value> {
    record[list]
        .as_array()
        .and_then(|rows| rows.iter().find(|e| is_runner_row(e, name)))
}

/// Put `entry` in `record[list]`. The runner row already there for the same
/// corpus is moved to `record.superseded` (with the date it was superseded),
/// never dropped: the history of what the record said is part of the record.
/// `corpora` also holds the labelled dead record other tests read, under the
/// same corpus name; a runner row never displaces it.
fn supersede(record: &mut Value, list: &str, name: &str, entry: Value, date: &str) {
    if !record[list].is_array() {
        record[list] = json!([]);
    }
    let items = record[list].as_array_mut().expect("checked above");
    let old = items
        .iter()
        .position(|e| is_runner_row(e, name))
        .map(|i| items.remove(i));
    items.push(entry);
    if let Some(old) = old {
        if !record["superseded"].is_array() {
            record["superseded"] = json!([]);
        }
        record["superseded"]
            .as_array_mut()
            .expect("checked above")
            .push(json!({ "list": list, "superseded_on": date, "entry": old }));
    }
}

/// Rewrites `record.open_gates` from the record itself.
fn refresh_open_gates(record: &mut Value, pins: &[Pin], gates: &Gates) {
    record["open_gates"] = json!(open_gates(record, pins, gates));
}

#[test]
#[ignore = "needs DEVMAP_CORPUS_<NAME> checkouts of the owner's private repositories"]
fn score_the_pinned_corpora_and_write_the_record() {
    let (pins, gates) = load_pins();
    let includes = verify_devmap_includes(&gates.must_include).unwrap_or_else(|e| panic!("{e}"));
    let mut record = load_record();
    let mut failures = Vec::new();
    let mut ran = 0;
    let date = today();

    for pin in &pins {
        let Some(dir) = std::env::var_os(env_name(&pin.name)).map(PathBuf::from) else {
            eprintln!("skipping {}: {} is not set", pin.name, env_name(&pin.name));
            continue;
        };
        ran += 1;
        let (measurement, labelled) =
            measure_corpus(&dir, pin, &includes).unwrap_or_else(|e| panic!("{}: {e}", pin.name));
        println!("{}", serde_json::to_string_pretty(&measurement).unwrap());

        let previous = runner_row(&record, "corpora", &pin.name).cloned();
        let labelled = carry_unverifiable(previous.as_ref(), labelled, pin);
        // A corpus without labels is gated too: every labelled gate then
        // reports itself unmeasured, which fails. Skipping it would let a run
        // with no labels anywhere pass every gate.
        for lang in &pin.languages {
            let prev_row = previous
                .as_ref()
                .map(|p| &p["languages"][lang.as_str()])
                .filter(|v| !v.is_null());
            let current = labelled
                .as_ref()
                .map_or(Value::Null, |e| e["languages"][lang.as_str()].clone());
            let lang_measurement =
                Some(&measurement["languages"][lang.as_str()]).filter(|v| !v.is_null());
            failures.extend(
                evaluate_gates(lang, prev_row, &current, lang_measurement, &gates)
                    .into_iter()
                    .map(|f| format!("{}: {f}", pin.name)),
            );
        }
        supersede(&mut record, "measurements", &pin.name, measurement, &date);
        if let Some(entry) = labelled {
            supersede(&mut record, "corpora", &pin.name, entry, &date);
        }
    }
    assert!(
        ran > 0,
        "no DEVMAP_CORPUS_<NAME> variable is set; nothing was measured"
    );

    // The record is written before the gates are asserted: a failing gate is a
    // finding to commit and explain, not a number to hide. `open_gates` carries
    // it in the record for every pinned corpus, measured this run or not.
    refresh_open_gates(&mut record, &pins, &gates);
    std::fs::write(
        record_path(),
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .expect("write record.json");
    assert!(
        failures.is_empty(),
        "record gates failed:\n  {}",
        failures.join("\n  ")
    );
}

/// CI half of the record's honesty: `record.json.open_gates` must be exactly
/// what the gates find in the committed record. So the record cannot claim a
/// clean bill it does not have - a stale row, an unlabelled corpus or a
/// nonzero AmbiguousGlobal count is listed until a run actually fixes it - and
/// a hand edit to `corpora.json` or the record that changes the outcome fails
/// here until `open_gates` says so.
/// `DEVMAP_ACCURACY_BLESS=1` rewrites `open_gates` from the record.
#[test]
fn the_committed_record_lists_every_gate_it_does_not_meet() {
    let (pins, gates) = load_pins();
    let mut record: Value = serde_json::from_str(
        &std::fs::read_to_string(record_path()).expect("record.json is readable"),
    )
    .expect("record.json is JSON");
    let found = open_gates(&record, &pins, &gates);
    if std::env::var_os("DEVMAP_ACCURACY_BLESS").is_some() {
        refresh_open_gates(&mut record, &pins, &gates);
        std::fs::write(
            record_path(),
            serde_json::to_string_pretty(&record).unwrap() + "\n",
        )
        .expect("write record.json");
        return;
    }
    let listed: Vec<String> = record["open_gates"]
        .as_array()
        .unwrap_or_else(|| panic!("record.json has no open_gates array"))
        .iter()
        .map(|v| v.as_str().expect("open_gates holds strings").to_string())
        .collect();
    let found_set: BTreeSet<&String> = found.iter().collect();
    let listed_set: BTreeSet<&String> = listed.iter().collect();
    let unlisted: Vec<&&String> = found_set.difference(&listed_set).collect();
    let cleared: Vec<&&String> = listed_set.difference(&found_set).collect();
    assert!(
        unlisted.is_empty() && cleared.is_empty(),
        "record.json open_gates is out of date (DEVMAP_ACCURACY_BLESS=1 rewrites it):\n  failing but not listed: {unlisted:#?}\n  listed but no longer failing: {cleared:#?}"
    );
}

// ----------------------------------------------- the scorer, in CI, unlabelled corpus

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-corpus-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A committed git checkout of the Go fixture, standing in for a corpus.
fn go_fixture_checkout(label: &str) -> (PathBuf, String) {
    let dir = scratch(label);
    copy_tree(&accuracy_root().join("go"), &dir);
    let run = |args: &[&str]| git(&dir, args).unwrap();
    run(&["init", "-q"]);
    run(&["add", "-A"]);
    run(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "-m",
        "fixture",
    ]);
    let head = run(&["rev-parse", "HEAD"]);
    (dir, head)
}

#[test]
fn the_scorer_buckets_a_coverage_capped_dead_row_with_the_confident_tier() {
    // Kotlin's ChronosFlow proof rows publish at 0.663 with the coverage-loss
    // reason: confident-branch evidence that the coverage cap only demoted.
    let row = |confidence: f32, reason: Option<&str>| DeadSymbolReport {
        symbol_name: "helper".into(),
        file_path: "src/a.kt".into(),
        confidence,
        is_exempt: false,
        exemption_reason: reason.map(str::to_string),
        evidence: None,
    };
    assert_eq!(tier_label(&row(0.663, Some(COVERAGE_LOSS_REASON))), "0.9");
    assert_eq!(tier_label(&row(0.4, Some(COVERAGE_LOSS_REASON))), "0.9");
    assert_eq!(tier_label(&row(0.9, None)), "0.9");
    assert_eq!(tier_label(&row(0.663, None)), "0.4");
}

#[test]
fn the_scorer_does_not_reimplement_the_confident_branch() {
    let source = include_str!("real_corpus_accuracy.rs");
    for needle in [
        ["starts_with(CONFIDENT", "_TIER_UNPROVEN)"],
        ["> f64::from(HIGHEST", "_DEGRADED_CONFIDENCE)"],
    ] {
        assert!(
            !source.contains(&needle.concat()),
            "{}: the confident-branch rule lives in devmap_analyze::dead_tier",
            needle.concat()
        );
    }
}

#[test]
fn the_scorer_refuses_a_commit_that_is_not_the_pinned_one() {
    let (dir, head) = go_fixture_checkout("pin");
    let pin = Pin {
        name: "fixture".into(),
        commit: head.clone(),
        languages: vec!["go".into()],
    };
    assert!(verify_pin(&dir, &pin).is_ok());

    let wrong = Pin {
        commit: "0".repeat(40),
        ..pin.clone()
    };
    let err = verify_pin(&dir, &wrong).unwrap_err();
    assert!(err.contains("not the pinned"), "{err}");

    // A tracked file edited after the pin: not the pinned tree either.
    std::fs::write(dir.join("go.work"), "go 1.21\n// edited\n").unwrap();
    let err = verify_pin(&dir, &pin).unwrap_err();
    assert!(err.contains("uncommitted changes"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_scorer_measures_structure_without_labels_and_scores_labels_when_given() {
    let (dir, head) = go_fixture_checkout("score");
    let pin = Pin {
        name: "fixture".into(),
        commit: head,
        languages: vec!["go".into()],
    };
    let built = common::build(&dir);

    let unlabelled = structure(&built, &pin.languages);
    let go = &unlabelled["go"];
    assert!(go["use_edges"].as_u64().unwrap() > 50, "{go}");
    assert_eq!(go["ambiguous_global_edges"].as_u64(), Some(0), "{go}");

    // `Caller01 -> Target01` is a correct, deterministic edge; `Caller02` has a
    // call the Go fixture records as unresolved, not as an edge.
    let labels = parse_labels(
        &json!({
            "edges": [{ "language": "go", "caller": "app/svc/svc.go::Caller01", "callee": "app/util/util.go::Target01", "correct": true }],
            "recall": [
                { "language": "go", "construct": "plain", "caller": "app/svc/svc.go::Caller01", "name": "Target01", "truth": "app/util/util.go::Target01" },
                { "language": "go", "construct": "pointer receiver", "caller": "app/svc/svc.go::Caller03", "name": "PtrGet03", "truth": "app/store/store.go::Store.PtrGet03" }
            ],
            // Every confident-branch dead row must carry a label (dead.tsv's
            // two truly_dead rows); DeadUtil is exempt and its label unused.
            // The fixture is not DevCouncil's own corpus, so each label
            // carries the hash and category its record row is written from.
            "dead": [
                { "language": "go", "id": "app/util/util2.go::DeadUtil", "truly_dead": true,
                  "symbol_hash": "h:0123456789ab", "category": "rg_declaration_only" },
                { "language": "go", "id": "app/svc/probes.go::deadPriv", "truly_dead": true,
                  "symbol_hash": "h:0123456789ac", "category": "rg_declaration_only" },
                { "language": "go", "id": "app/svc/probes.go::fast.unusedM", "truly_dead": true,
                  "symbol_hash": "h:0123456789ad", "category": "rg_declaration_only" }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let scored = score_labels("fixture", &built, &labels, &pin.languages).unwrap();
    let go = &scored["go"];
    assert_eq!(go["confident_edge_precision"]["correct"], 1);
    assert_eq!(go["recall"]["total"], 2);
    // `Caller03` (`var s store.Store; s.PtrGet03()`) was a recorded miss until the
    // Go type evaluator typed a `var x pkg.T`; both labelled edges are now present.
    assert_eq!(
        go["recall"]["present"], 2,
        "Caller03 binds through the typed receiver: {go}"
    );
    assert_eq!(go["honest_miss"]["absent"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_label_that_matches_no_edge_is_an_error_not_a_miss() {
    let (dir, _) = go_fixture_checkout("stale");
    let built = common::build(&dir);
    let labels = parse_labels(
        &json!({ "edges": [{ "language": "go", "caller": "app/gone.go::F", "callee": "app/gone.go::G", "correct": true }] })
            .to_string(),
    )
    .unwrap();
    let err = score_labels("fixture", &built, &labels, &["go".to_string()]).unwrap_err();
    assert!(err.contains("matches no edge"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

const PRIVATE_SYMBOL: &str = "app/svc/probes.go::deadPriv";

/// The fixture's other confident-branch dead row, labelled in full: a language's
/// dead labels must cover its whole confident branch, so a test of one label's
/// shape still labels the second row.
fn companion_dead_label() -> Value {
    json!({
        "language": "go", "id": "app/svc/probes.go::fast.unusedM", "truly_dead": true,
        "symbol_hash": "h:0123456789ff", "category": "rg_declaration_only",
        "evidence": "rg -uu -w unusedM: declaration only",
    })
}

/// dead.tsv's two truly_dead rows, the fixture's whole confident branch, as a
/// private corpus's labels (hash and category, no raw prose).
fn fixture_dead_labels() -> Vec<Value> {
    vec![
        json!({ "language": "go", "id": "app/svc/probes.go::deadPriv", "truly_dead": true,
                "symbol_hash": "h:0123456789ab", "category": "rg_declaration_only" }),
        json!({ "language": "go", "id": "app/svc/probes.go::fast.unusedM", "truly_dead": true,
                "symbol_hash": "h:0123456789ac", "category": "rg_declaration_only" }),
    ]
}

fn dead_labels(extra: Value) -> Labels {
    let mut label = json!({ "language": "go", "id": PRIVATE_SYMBOL, "truly_dead": true });
    for (k, v) in extra.as_object().unwrap() {
        label[k] = v.clone();
    }
    parse_labels(&json!({ "dead": [label, companion_dead_label()] }).to_string()).unwrap()
}

fn score_dead(corpus: &str, labels: &Labels) -> Result<BTreeMap<String, Value>, String> {
    // The tests run in parallel and `scratch` names a directory by label and
    // process, so each call needs a label of its own.
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (dir, _) = go_fixture_checkout(&format!("identity-{n}"));
    let built = common::build(&dir);
    let scored = score_labels(corpus, &built, labels, &["go".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
    scored
}

#[test]
fn a_private_corpus_row_is_written_hashed_with_a_category_never_the_raw_symbol() {
    let labels = dead_labels(json!({
        "symbol_hash": "h:0123456789ab",
        "category": "rg_declaration_only",
        "evidence": "rg -uu -w deadPriv: app/svc/probes.go declaration only",
    }));
    let scored = score_dead("Private", &labels).unwrap();
    let rows = scored["go"]["labelled_rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{}", scored["go"]);
    assert_eq!(rows[0]["symbol"], "h:0123456789ab");
    assert_eq!(rows[0]["evidence"], "rg_declaration_only");
    assert_eq!(rows[0]["verdict"], "dead_true");
    let written = scored["go"].to_string();
    assert!(
        !written.contains("probes") && !written.contains("deadPriv") && !written.contains("rg -uu"),
        "the raw symbol or the labeller's prose reached the record: {written}"
    );
}

#[test]
fn a_private_label_without_a_hash_or_a_category_is_reported_malformed_naming_the_label_not_its_text(
) {
    for (missing, extra) in [
        ("symbol_hash", json!({ "category": "rg_declaration_only" })),
        ("category", json!({ "symbol_hash": "h:0123456789ab" })),
        (
            "symbol_hash",
            json!({ "symbol_hash": PRIVATE_SYMBOL, "category": "rg_declaration_only" }),
        ),
        (
            "symbol_hash",
            json!({ "symbol_hash": "h:0123456789AB", "category": "rg_declaration_only" }),
        ),
        (
            "category",
            json!({ "symbol_hash": "h:0123456789ab", "category": "nothing imports it" }),
        ),
        (
            "category",
            json!({ "symbol_hash": "h:0123456789ab", "category": "src/a.rs:12" }),
        ),
    ] {
        // The run goes on: the label is reported, not an error.
        let scored = score_dead("Private", &dead_labels(extra.clone()))
            .unwrap_or_else(|e| panic!("{extra}: a malformed label aborted the run: {e}"));
        let go = &scored["go"];
        let um = &go["dead_unmeasured"];
        assert_eq!(um["malformed"], 1, "{extra}: {go}");
        let reason = um["malformed_sample"][0].as_str().unwrap();
        assert!(
            reason.contains("Private go dead label 0") && reason.contains(missing),
            "{extra}: {reason}"
        );
        let written = go.to_string();
        assert!(
            !written.contains("probes") && !written.contains("nothing imports"),
            "{written}"
        );
        // Unmeasured, never scored: the malformed row is in neither the rows
        // nor the rate, and the dead gate cannot read the row as a pass.
        assert_eq!(go["labelled_rows"].as_array().unwrap().len(), 1, "{go}");
        assert_eq!(go["dead_fp_by_tier"]["0.9"]["rows"], 1, "{go}");
        assert_eq!(um["unlabelled"], 0, "counted once, as malformed: {go}");
        assert_eq!(dead_fp_rate(go), None, "{go}");
    }
}

#[test]
fn a_malformed_label_lappi_style_does_not_abort_the_run() {
    // A label with no `symbol_hash` at all (Lappi's shape) used to make the
    // whole run return Err; it is now one malformed, unmeasured row.
    let labels = dead_labels(json!({ "category": "rg_declaration_only" }));
    let scored = score_dead("Lappi", &labels).expect("the run keeps going");
    assert_eq!(scored["go"]["dead_unmeasured"]["malformed"], 1);
    assert!(
        regression_gates("go", None, &scored["go"], &BTreeMap::new())
            .iter()
            .any(|f| f.contains("dead false-positive rate is unmeasured")),
        "an unmeasured dead rate must be reported as a failed gate"
    );
}

#[test]
fn a_label_that_matches_no_row_is_still_checked() {
    let mut dead = vec![
        json!({ "language": "go", "id": PRIVATE_SYMBOL, "truly_dead": true,
                "symbol_hash": "h:0123456789ab", "category": "rg_declaration_only" }),
        companion_dead_label(),
    ];
    dead.push(json!({ "language": "go", "id": "app/gone.go::F", "truly_dead": true }));
    let labels = parse_labels(&json!({ "dead": dead }).to_string()).unwrap();
    let scored = score_dead("Private", &labels).expect("a malformed label does not abort");
    let um = &scored["go"]["dead_unmeasured"];
    assert_eq!(um["malformed"], 1, "{um}");
    assert!(
        um["malformed_sample"][0]
            .as_str()
            .unwrap()
            .contains("Private go dead label 2"),
        "{um}"
    );
}

#[test]
fn devcouncils_own_rows_keep_their_symbol_and_evidence() {
    let own = dead_labels(json!({ "evidence": "rg -uu -w deadPriv: declaration only" }));
    let scored = score_dead(OWN_CORPUS, &own).unwrap();
    let rows = scored["go"]["labelled_rows"].as_array().unwrap();
    assert_eq!(rows[0]["symbol"], PRIVATE_SYMBOL);
    assert_eq!(rows[0]["evidence"], "rg -uu -w deadPriv: declaration only");
    // Its own corpus needs no hash, and still needs its evidence.
    let missing = score_dead(OWN_CORPUS, &dead_labels(json!({}))).unwrap();
    assert_eq!(missing["go"]["dead_unmeasured"]["malformed"], 1);
}

#[test]
fn an_unsure_label_is_listed_and_counted_but_scored_neither_way() {
    let labels = dead_labels(json!({
        "symbol_hash": "h:0123456789ab",
        "category": "spot_check_upstream_shadow",
        "unsure": true,
    }));
    let scored = score_dead("Private", &labels).unwrap();
    let go = &scored["go"];
    assert_eq!(go["labelled_rows"][0]["verdict"], "unsure");
    assert_eq!(go["unsure"], 1);
    // Only the companion row is scored: the unsure one counts toward neither
    // side of the rate.
    assert_eq!(
        go["dead_fp_by_tier"],
        json!({ "0.9": { "false_positive": 0, "rows": 1 } }),
        "{go}"
    );
}

#[test]
fn the_identity_shape_is_not_redefined_here() {
    let source = include_str!("real_corpus_accuracy.rs");
    let own_copy = ["strip_prefix(\"h", ":\")"].concat();
    assert!(
        !source.contains(&own_copy),
        "the hash shape lives in devmap_analyze::record_identity"
    );
}

/// `corpora` holds two kinds of row under one corpus name: the hand-labelled
/// dead record (`commit`, read by `dead_tier_eligibility.rs`) and the rows this
/// runner writes (`corpus_commit`). A runner row replaces only the runner row
/// before it; the dead record is never superseded by one.
#[test]
fn a_runner_row_never_supersedes_the_labelled_dead_record() {
    let dead_record = json!({
        "name": "x",
        "commit": "c1",
        "languages": { "go": { "dead_fp_by_tier": { "0.9": { "false_positive": 0, "rows": 2 } } } },
    });
    let mut record = json!({ "corpora": [dead_record.clone()] });
    let first = json!({ "name": "x", "corpus_commit": "p1", "languages": {} });
    supersede(&mut record, "corpora", "x", first.clone(), "d1");
    assert_eq!(
        record["corpora"],
        json!([dead_record.clone(), first.clone()])
    );
    assert!(record["superseded"].is_null(), "{record}");
    assert_eq!(runner_row(&record, "corpora", "x"), Some(&first));

    let second = json!({ "name": "x", "corpus_commit": "p2", "languages": {} });
    supersede(&mut record, "corpora", "x", second.clone(), "d2");
    assert_eq!(record["corpora"], json!([dead_record, second]));
    assert_eq!(
        record["superseded"],
        json!([{ "list": "corpora", "superseded_on": "d2", "entry": first }])
    );
}

/// A pinned language the labels say nothing about gets no row: an empty row
/// would read as measured. A row that is written carries `dead_fp_by_tier.0.9`
/// (the shape `dead_tier_eligibility.rs` requires of every row). A
/// confident-branch dead row nobody labelled is counted in `dead_unmeasured`,
/// never quietly left out of `rows` and never a pass.
#[test]
fn a_scored_row_is_written_only_for_a_labelled_language_and_carries_the_confident_dead_tier() {
    let (dir, _) = go_fixture_checkout("rows");
    let built = common::build(&dir);
    let languages = vec!["go".to_string(), "typescript".to_string()];
    let recall = json!([{ "language": "go", "construct": "plain", "caller": "app/svc/svc.go::Caller01", "name": "Target01", "truth": "app/util/util.go::Target01" }]);
    let labels = parse_labels(
        &json!({
            "recall": recall,
            "dead": fixture_dead_labels()
        })
        .to_string(),
    )
    .unwrap();
    let scored = score_labels("Fixture", &built, &labels, &languages).unwrap();
    assert!(
        !scored.contains_key("typescript"),
        "no typescript label, so no typescript row: {scored:?}"
    );
    assert_eq!(
        scored["go"]["dead_fp_by_tier"]["0.9"],
        json!({ "false_positive": 0, "rows": 2 })
    );

    let partial = parse_labels(
        &json!({
            "recall": recall,
            "dead": fixture_dead_labels()[..1]
        })
        .to_string(),
    )
    .unwrap();
    // An unlabelled confident row no longer aborts the run: it is counted as
    // unmeasured, the labelled row is scored as before, and the dead gate
    // reads unmeasured rather than a rate over the subset.
    let scored = score_labels("Fixture", &built, &partial, &languages)
        .expect("an unlabelled confident row does not abort the run");
    let go = &scored["go"];
    assert_eq!(go["dead_unmeasured"]["unlabelled"], 1, "{go}");
    assert_eq!(go["dead_unmeasured"]["malformed"], 0, "{go}");
    assert_eq!(
        go["dead_fp_by_tier"]["0.9"],
        json!({ "false_positive": 0, "rows": 1 }),
        "the labelled row scores exactly as before: {go}"
    );
    assert_eq!(dead_fp_rate(go), None, "an unlabelled row is never a pass");
    // DevCouncil's own rows carry the id sample; a private corpus's do not.
    assert_eq!(
        go["dead_unmeasured"]["unlabelled_sample"],
        json!([]),
        "{go}"
    );
    let own_labels = parse_labels(
        &json!({
            "recall": recall,
            "dead": [{ "language": "go", "id": "app/svc/probes.go::deadPriv",
                       "truly_dead": true, "evidence": "rg -uu -w deadPriv: declaration only" }]
        })
        .to_string(),
    )
    .unwrap();
    let own = score_labels(OWN_CORPUS, &built, &own_labels, &languages).unwrap();
    assert_eq!(
        own["go"]["dead_unmeasured"]["unlabelled_sample"],
        json!(["app/svc/probes.go::fast.unusedM"]),
        "{}",
        own["go"]
    );
    // The fully labelled fixture reports nothing unmeasured and has a rate.
    let full = score_labels("Fixture", &built, &labels, &languages).unwrap();
    assert_eq!(dead_unmeasured_count(&full["go"]), 0, "{}", full["go"]);
    assert_eq!(dead_fp_rate(&full["go"]), Some(0.0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The committed Go oracle labels (`go.labels.json`, written by
/// `tools/accuracy/cmd/golabel -root rust/testdata/accuracy/go`) scored against
/// the fixture build: every id the oracle names is one the build has, so the
/// labeller and DevMap agree on symbol ids, and the fixture - no Go row in
/// `known_failing.tsv` - scores perfect precision, recall and honest-miss.
/// The labeller's own golden test keeps the file equal to its output.
#[test]
fn the_go_oracle_labels_name_the_fixture_build_and_score_it_clean() {
    let (dir, _) = go_fixture_checkout("golabel");
    let built = common::build(&dir);
    let path = accuracy_root().join("go.labels.json");
    let mut raw: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // dead.tsv's two truly_dead rows, the fixture's whole confident branch.
    raw["dead"] = json!(fixture_dead_labels());
    let labels = parse_labels(&raw.to_string()).unwrap();

    let known: BTreeSet<&str> = built
        .extractions
        .iter()
        .flat_map(|x| {
            std::iter::once(x.file_path.as_str())
                .chain(x.symbols.iter().map(|s| s.qualified_name.as_str()))
        })
        .collect();
    let mut unknown: BTreeSet<&str> = BTreeSet::new();
    for l in &labels.recall {
        unknown.extend(
            std::iter::once(l.caller.as_str())
                .chain(l.truth.split('|'))
                .filter(|id| !known.contains(id)),
        );
    }
    for o in &labels.oracle {
        unknown.extend(
            o.covered_callers
                .iter()
                .map(String::as_str)
                .filter(|id| !known.contains(id)),
        );
        unknown.extend(
            o.uses
                .iter()
                .map(|(_, callee)| callee.as_str())
                .filter(|id| !known.contains(id)),
        );
    }
    assert!(
        unknown.is_empty(),
        "the oracle names ids the build does not have, so its id scheme and DevMap's disagree: {unknown:#?}"
    );

    let scored = score_labels("Fixture", &built, &labels, &["go".to_string()]).unwrap();
    let go = &scored["go"];
    let precision = &go["confident_edge_precision"];
    assert!(precision["total"].as_u64().unwrap() > 50, "{go}");
    assert_eq!(precision["correct"], precision["total"], "{go}");
    let unique = &go["unique_global_precision"];
    assert_eq!(unique["correct"], unique["total"], "{go}");
    assert!(go["recall"]["total"].as_u64().unwrap() > 50, "{go}");
    assert_eq!(go["recall"]["present"], go["recall"]["total"], "{go}");
    assert_eq!(go["honest_miss"]["absent"], 0, "{go}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An oracle judges every confident Calls/References edge from a caller it
/// read, right when it bound the same pair and wrong when it did not; an edge
/// from a caller it did not read is not judged at all.
#[test]
fn an_oracle_judges_every_confident_edge_from_a_caller_it_read_and_no_other() {
    let (dir, _) = go_fixture_checkout("oracle");
    let built = common::build(&dir);
    let go = vec!["go".to_string()];
    let dead = fixture_dead_labels();
    let score = |oracle: Value| {
        let labels =
            parse_labels(&json!({ "dead": dead, "oracle": [oracle] }).to_string()).unwrap();
        score_labels("Fixture", &built, &labels, &go).unwrap()["go"]["confident_edge_precision"]
            .clone()
    };

    let agrees = score(json!({
        "language": "go",
        "covered_callers": ["app/svc/svc.go::Caller01"],
        "uses": [{ "caller": "app/svc/svc.go::Caller01", "callee": "app/util/util.go::Target01", "impls": [] }]
    }));
    assert_eq!(agrees["total"], 1, "{agrees}");
    assert_eq!(agrees["correct"], 1, "{agrees}");

    // The same caller, an oracle that bound nothing: DevMap's edge is wrong.
    let disagrees = score(json!({
        "language": "go",
        "covered_callers": ["app/svc/svc.go::Caller01"],
        "uses": []
    }));
    assert_eq!(disagrees["total"], 1, "{disagrees}");
    assert_eq!(disagrees["correct"], 0, "{disagrees}");

    // A caller the oracle never read: nothing to judge.
    let unread = score(json!({ "language": "go", "covered_callers": [], "uses": [] }));
    assert_eq!(unread["total"], 0, "{unread}");

    // A use from a caller the oracle says it did not read is malformed.
    let err = parse_labels(
        &json!({ "oracle": [{
            "language": "go",
            "covered_callers": [],
            "uses": [{ "caller": "app/svc/svc.go::Caller01", "callee": "app/util/util.go::Target01", "impls": [] }]
        }] })
        .to_string(),
    )
    .unwrap_err();
    assert!(err.contains("not in covered_callers"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn row(precision: (u64, u64), recall: (u64, u64), honest: (u64, u64), dead: (u64, u64)) -> Value {
    let rate = |(a, b): (u64, u64)| (b > 0).then(|| a as f64 / b as f64);
    json!({
        "confident_edge_precision": { "correct": precision.0, "total": precision.1, "rate": rate(precision) },
        "recall": { "present": recall.0, "total": recall.1, "rate": rate(recall), "by_construct": {} },
        "honest_miss": { "recorded": honest.0, "absent": honest.1, "rate": rate(honest) },
        "dead_fp_by_tier": { "0.9": { "false_positive": dead.0, "rows": dead.1 } },
    })
}

fn floors() -> BTreeMap<String, f64> {
    load_pins().1.floors
}

fn gates() -> Gates {
    load_pins().1
}

#[test]
fn a_gate_that_held_must_keep_holding_and_one_that_did_not_may_only_stay_put() {
    // First record, everything below the floors: the recall floor is only a
    // ratchet, so a first recall below it is a baseline; the absolute gates
    // (precision, honest-miss, dead FP) are open whatever `previous` says.
    let first = row((90, 100), (60, 100), (90, 100), (3, 10));
    let found = regression_gates("go", None, &first, &floors());
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(!found.iter().any(|f| f.contains("recall")), "{found:?}");

    // The same numbers again: unchanged is not a regression, only still open.
    let found = regression_gates("go", Some(&first), &first, &floors());
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(!found.iter().any(|f| f.contains("regressed")), "{found:?}");

    // Precision fell two points: a regression even though it never met 0.95.
    let worse = row((88, 100), (60, 100), (90, 100), (3, 10));
    let found = regression_gates("go", Some(&first), &worse, &floors());
    assert!(
        found
            .iter()
            .any(|f| f.contains("confident edge precision regressed")),
        "{found:?}"
    );

    // Met the floor once, must meet it again.
    let good = row((97, 100), (90, 100), (99, 100), (0, 10));
    assert!(regression_gates("go", Some(&good), &good, &floors()).is_empty());
    let slipped = row((94, 100), (90, 100), (99, 100), (0, 10));
    let found = regression_gates("go", Some(&good), &slipped, &floors());
    assert!(
        found.iter().any(|f| f.contains("no longer meets")),
        "{found:?}"
    );

    // Recall under the Go floor (88) that previously held.
    let low_recall = row((97, 100), (80, 100), (99, 100), (0, 10));
    let found = regression_gates("go", Some(&good), &low_recall, &floors());
    assert!(found.iter().any(|f| f.contains("recall")), "{found:?}");

    // Dead false positives rising is a regression with zero tolerance.
    let dead_worse = row((97, 100), (90, 100), (99, 100), (1, 10));
    let found = regression_gates("go", Some(&good), &dead_worse, &floors());
    assert!(
        found.iter().any(|f| f.contains("dead false-positive")),
        "{found:?}"
    );
}

#[test]
fn an_absolute_floor_is_open_on_a_first_measurement_and_a_recall_floor_is_not() {
    // The scholarlm TypeScript shape: honest-miss 26/27 = 96.3% against 98%,
    // with no previous row.
    let first = row((97, 100), (90, 100), (26, 27), (0, 10));
    let found = regression_gates("typescript", None, &first, &floors());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("honest-miss rate 0.963 does not meet the absolute floor 0.980"),
        "{found:?}"
    );

    // Each absolute gate on its own, with no previous row.
    let found = regression_gates(
        "go",
        None,
        &row((94, 100), (90, 100), (99, 100), (0, 10)),
        &floors(),
    );
    assert!(
        found
            .iter()
            .any(|f| f.contains("confident edge precision 0.940")),
        "{found:?}"
    );
    let found = regression_gates(
        "go",
        None,
        &row((97, 100), (90, 100), (99, 100), (1, 10)),
        &floors(),
    );
    assert!(
        found
            .iter()
            .any(|f| f.contains("dead false-positive rate 0.100")),
        "{found:?}"
    );

    // Exactly at each threshold is met, not open.
    let at = row((95, 100), (88, 100), (98, 100), (1, 20));
    assert!(regression_gates("go", None, &at, &floors()).is_empty());

    // Previous met, current does not: "no longer meets" plus the regression,
    // and not also the absolute-floor line.
    let good = row((97, 100), (90, 100), (99, 100), (0, 10));
    let slipped = row((97, 100), (90, 100), (96, 100), (0, 10));
    let found = regression_gates("go", Some(&good), &slipped, &floors());
    assert_eq!(
        found.iter().filter(|f| f.contains("honest-miss")).count(),
        2,
        "no longer meets + regressed: {found:?}"
    );
    assert!(
        found.iter().any(|f| f.contains("no longer meets")),
        "{found:?}"
    );
    assert!(
        !found.iter().any(|f| f.contains("absolute floor")),
        "{found:?}"
    );

    // Previous unmet and current unmet: still open.
    let prev = row((97, 100), (90, 100), (95, 100), (0, 10));
    let found = regression_gates("go", Some(&prev), &slipped, &floors());
    assert!(
        found.iter().any(|f| f.contains("absolute floor")),
        "{found:?}"
    );

    // n = 0: unmeasured, not passed and not a panic.
    let found = regression_gates(
        "go",
        None,
        &row((97, 100), (90, 100), (0, 0), (0, 10)),
        &floors(),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("honest-miss rate is unmeasured"),
        "{found:?}"
    );
}

#[test]
fn an_unmeasured_gate_is_reported_and_never_passes() {
    let unlabelled = row((0, 0), (0, 0), (0, 0), (0, 0));
    let found = evaluate_gates("python", None, &unlabelled, None, &gates());
    assert!(
        found
            .iter()
            .any(|f| f.contains("confident edge precision is unmeasured")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|f| f.contains("honest-miss rate is unmeasured")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|f| f.contains("recall is unmeasured")),
        "{found:?}"
    );
    for claim in [
        "recall card target 0.880 is unmeasured",
        "UniqueGlobal precision is unmeasured",
        "AmbiguousGlobal edges are unmeasured",
    ] {
        assert!(
            found.iter().any(|f| f.contains(claim)),
            "{claim}: {found:?}"
        );
    }
    // A corpus with no labels at all is `Null`, and fails the same way.
    let found = evaluate_gates("go", None, &Value::Null, None, &gates());
    assert!(
        found
            .iter()
            .any(|f| f.contains("confident edge precision is unmeasured")),
        "{found:?}"
    );
}

/// A label-free measurement row for one language.
fn measured(ambiguous: u64, unique: u64) -> Value {
    json!({
        "ambiguous_global_edges": ambiguous,
        "use_edges_by_resolution": { "AmbiguousGlobal": ambiguous, "UniqueGlobal": unique, "SameFile": 10 },
    })
}

/// A labelled row that meets every claim for Go (target 95).
fn meeting_go_claims() -> Value {
    let mut r = row((97, 100), (96, 100), (99, 100), (0, 10));
    r["unique_global_precision"] = json!({ "correct": 19, "total": 20, "rate": 0.95 });
    r
}

#[test]
fn the_claim_gates_pass_only_when_every_claim_is_measured_and_met() {
    let m = measured(0, 20);
    let found = claim_gates("go", &meeting_go_claims(), Some(&m), &gates());
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn ambiguous_global_edges_fail_the_gate_whatever_the_previous_record_said() {
    let found = claim_gates(
        "go",
        &meeting_go_claims(),
        Some(&measured(13, 20)),
        &gates(),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("13 AmbiguousGlobal edge(s)"), "{found:?}");
    // Absolute: an unchanged nonzero count against an identical previous row
    // still fails through evaluate_gates.
    let r = meeting_go_claims();
    let found = evaluate_gates("go", Some(&r), &r, Some(&measured(13, 20)), &gates());
    assert!(
        found.iter().any(|f| f.contains("AmbiguousGlobal")),
        "{found:?}"
    );
}

#[test]
fn unique_global_precision_is_gated_at_095_and_vacuous_only_with_no_unique_global_edge() {
    let mut r = meeting_go_claims();
    r["unique_global_precision"] = json!({ "correct": 18, "total": 20, "rate": 0.9 });
    let found = claim_gates("go", &r, Some(&measured(0, 20)), &gates());
    assert!(
        found
            .iter()
            .any(|f| f.contains("UniqueGlobal precision 0.900 is below 0.950")),
        "{found:?}"
    );

    r["unique_global_precision"] = json!({ "correct": 0, "total": 0, "rate": null });
    let found = claim_gates("go", &r, Some(&measured(0, 20)), &gates());
    assert!(
        found
            .iter()
            .any(|f| f.contains("UniqueGlobal precision is unmeasured")),
        "20 UniqueGlobal edges and no label on any: {found:?}"
    );
    let found = claim_gates("go", &r, Some(&measured(0, 0)), &gates());
    assert!(found.is_empty(), "no UniqueGlobal edge to judge: {found:?}");
}

#[test]
fn a_card_target_is_absolute_and_separate_from_the_baseline_floor() {
    // 90% clears the Go floor (88) but not the Go card's target (95).
    let mut r = meeting_go_claims();
    r["recall"] = json!({ "present": 90, "total": 100, "rate": 0.9, "by_construct": {} });
    assert!(regression_gates("go", None, &r, &floors()).is_empty());
    let found = claim_gates("go", &r, Some(&measured(0, 20)), &gates());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("recall 0.900 is below the card target 0.950"),
        "{found:?}"
    );

    // Rust: each class against its own target (95 / 75).
    let mut rust = meeting_go_claims();
    rust["recall"]["by_construct"] = json!({
        "free_self": { "correct": 96, "total": 100, "rate": 0.96 },
        "non_self": { "correct": 70, "total": 100, "rate": 0.70 },
    });
    let found = claim_gates("rust", &rust, Some(&measured(0, 20)), &gates());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("recall[non_self] 0.700"), "{found:?}");
}

#[test]
fn a_language_with_no_recall_entry_is_reported_as_ungated() {
    let mut g = gates();
    g.targets.remove("kotlin");
    let found = claim_gates("kotlin", &meeting_go_claims(), Some(&measured(0, 20)), &g);
    assert!(found.iter().any(|f| f.contains("ungated")), "{found:?}");
    let found = regression_gates("cobol", None, &meeting_go_claims(), &g.floors);
    assert!(found.iter().any(|f| f.contains("ungated")), "{found:?}");
}

#[test]
fn no_recall_floor_or_card_target_is_ever_lowered() {
    let g = gates();
    for (table, minimums, what) in [
        (&g.floors, &MIN_RECALL_FLOORS_PERCENT, "floor"),
        (&g.targets, &MIN_RECALL_TARGETS_PERCENT, "card target"),
    ] {
        for (key, min) in minimums {
            let now = table[*key] * 100.0;
            assert!(
                now + 1e-9 >= *min,
                "corpora.json lowers the {key} recall {what} to {now} (minimum {min}); a {what} \
                 only ratchets up, in the change that earns it"
            );
        }
    }
    for key in RECALL_KEYS {
        assert!(
            g.targets[key] + 1e-9 >= g.floors[key],
            "{key}: card target {} is below its floor {}",
            g.targets[key],
            g.floors[key]
        );
    }
}

#[test]
fn the_record_must_include_d0552ae1_and_every_language_lane() {
    let g = gates();
    for c in &g.must_include {
        assert_eq!(c.len(), 40, "{c}: a full commit id");
        assert!(c.chars().all(|ch| ch.is_ascii_hexdigit()), "{c}");
    }
    assert!(
        g.must_include
            .iter()
            .any(|c| c == "d0552ae16cc969a98d2055bd90efba473306c278"),
        "S2 (d0552ae1) must be required of every record row"
    );
}

fn pin(name: &str, langs: &[&str]) -> Pin {
    Pin {
        name: name.into(),
        commit: "a".repeat(40),
        languages: langs.iter().map(|l| (*l).to_string()).collect(),
    }
}

#[test]
fn a_stale_or_missing_measurement_is_an_open_gate() {
    let g = gates();
    let p = pin("corpus", &["go"]);
    let mut record = json!({ "corpora": [], "measurements": [] });
    let open = open_gates(&record, std::slice::from_ref(&p), &g);
    assert!(
        open.iter().any(|f| f.contains("no label-free measurement")),
        "{open:?}"
    );

    record["measurements"] = json!([{
        "name": "corpus", "corpus_commit": "a".repeat(40), "devmap_commit": "40b6bbbf", "date": "2026-10-10",
        "languages": { "go": measured(0, 0) },
    }]);
    let open = open_gates(&record, std::slice::from_ref(&p), &g);
    assert!(open.iter().any(|f| f.contains("stale")), "{open:?}");

    record["measurements"][0]["includes_commits"] = json!(g.must_include);
    record["corpora"] = json!([{ "name": "corpus", "corpus_commit": "a".repeat(40), "languages": { "go": meeting_go_claims() } }]);
    let open = open_gates(&record, std::slice::from_ref(&p), &g);
    assert!(
        open.is_empty(),
        "fresh, labelled and meeting every claim: {open:?}"
    );

    // A labelled row at another corpus commit is not a measurement of the pin.
    record["corpora"][0]["corpus_commit"] = json!("b".repeat(40));
    let open = open_gates(&record, std::slice::from_ref(&p), &g);
    assert!(
        open.iter()
            .any(|f| f.contains("confident edge precision is unmeasured")),
        "{open:?}"
    );
}

#[test]
fn a_re_measured_corpus_supersedes_its_previous_row_and_keeps_it() {
    // Runner rows: each carries the `corpus_commit` it was measured at.
    let row = |name: &str, v: i64| json!({ "name": name, "corpus_commit": "p", "v": v });
    let mut record = json!({ "measurements": [row("a", 1), row("b", 1)] });
    supersede(&mut record, "measurements", "a", row("a", 2), "2026-10-13");
    supersede(&mut record, "measurements", "c", row("c", 1), "2026-10-13");
    let names: Vec<(&str, i64)> = record["measurements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["v"].as_i64().unwrap()))
        .collect();
    assert_eq!(names, [("b", 1), ("a", 2), ("c", 1)]);
    let old = record["superseded"].as_array().unwrap();
    assert_eq!(old.len(), 1, "only the replaced row moves: {old:?}");
    assert_eq!(old[0]["list"], "measurements");
    assert_eq!(old[0]["superseded_on"], "2026-10-13");
    assert_eq!(old[0]["entry"], row("a", 1));
}

#[test]
fn rust_recall_is_gated_per_class() {
    let mut r = row((97, 100), (90, 100), (99, 100), (0, 10));
    r["recall"]["by_construct"] = json!({
        "free_self": { "correct": 92, "total": 100, "rate": 0.92 },
        "non_self": { "correct": 40, "total": 100, "rate": 0.40 },
    });
    let mut prev = r.clone();
    prev["recall"]["by_construct"]["non_self"] =
        json!({ "correct": 50, "total": 100, "rate": 0.5 });
    let found = regression_gates("rust", Some(&prev), &r, &floors());
    assert!(
        found.iter().any(|f| f.contains("recall[non_self]")),
        "{found:?}"
    );
    assert!(
        !found.iter().any(|f| f.contains("recall[free_self]")),
        "{found:?}"
    );
}

#[test]
fn every_pinned_corpus_has_a_forty_hex_commit_and_a_language() {
    let (pins, gates) = load_pins();
    let floors = gates.floors;
    assert!(!pins.is_empty());
    for pin in pins {
        assert_eq!(pin.commit.len(), 40, "{} commit {:?}", pin.name, pin.commit);
        assert!(pin.commit.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!pin.languages.is_empty(), "{} names no language", pin.name);
    }
    for key in [
        "go",
        "python",
        "typescript",
        "rust_free_self",
        "rust_non_self",
        "swift",
        "kotlin",
    ] {
        assert!(floors.contains_key(key), "no recall floor for {key}");
    }
}

// ------------------------------------- honest-miss split, tiers, unverifiable

/// A site that received a wrong HIGH edge is one defect: precision debits it
/// once, and the honest-miss gate reports it as `wrongly_bound` instead of
/// counting it a second time as a silent miss.
#[test]
fn a_wrong_high_edge_is_counted_once_in_precision_and_as_wrongly_bound_not_silent() {
    let (dir, _) = go_fixture_checkout("wrong-high");
    let built = common::build(&dir);
    let ledger_row = built
        .resolution
        .unresolved
        .iter()
        .find(|u| UNATTRIBUTED_LABELS.contains(&u.class.label()))
        .expect("the Go fixture records an unattributed call in its ledger");
    let recorded_site = json!({
        "language": "go", "construct": "recorded", "caller": ledger_row.source_symbol,
        "name": ledger_row.callee_name, "truth": "app/util/util2.go::Target02",
    });
    let labels = parse_labels(
        &json!({
            "edges": [{ "language": "go", "caller": "app/svc/svc.go::Caller01", "callee": "app/util/util.go::Target01", "correct": false }],
            "recall": [
                // Caller01 is bound HIGH to Target01; the truth is said to be Target02.
                { "language": "go", "construct": "wrong", "caller": "app/svc/svc.go::Caller01", "name": "Target01", "truth": "app/util/util2.go::Target02" },
                // No edge and no ledger row for this name at all.
                { "language": "go", "construct": "silent", "caller": "app/svc/svc.go::Caller01", "name": "NoSuchCall", "truth": "app/util/util2.go::Target02" },
                recorded_site,
            ],
            "dead": fixture_dead_labels(),
        })
        .to_string(),
    )
    .unwrap();
    let scored = score_labels("Fixture", &built, &labels, &["go".to_string()]).unwrap();
    let go = &scored["go"];
    assert_eq!(go["confident_edge_precision"]["total"], 1, "{go}");
    assert_eq!(go["confident_edge_precision"]["correct"], 0, "{go}");
    let honest = &go["honest_miss"];
    assert_eq!(honest["wrongly_bound"], 1, "{honest}");
    assert_eq!(honest["silent"], 1, "{honest}");
    assert_eq!(honest["recorded"], 1, "{honest}");
    assert_eq!(honest["absent"], 3, "{honest}");
    // wrongly_bound is outside the denominator: 1 / (1 + 1).
    assert_eq!(honest["rate"], 0.5, "{honest}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn honest_miss_rate_ignores_wrongly_bound_sites_and_is_unmeasured_without_a_denominator() {
    let only_wrong = HonestMiss {
        wrongly_bound: 4,
        ..HonestMiss::default()
    };
    assert_eq!(only_wrong.rate(), None);
    assert_eq!(only_wrong.json()["absent"], 4);
    let mixed = HonestMiss {
        recorded: 3,
        silent: 1,
        wrongly_bound: 9,
    };
    assert_eq!(mixed.rate(), Some(0.75));
}

#[test]
fn the_dead_gate_tells_an_empty_tier_from_an_unmeasured_one_and_neither_passes() {
    let tier = |rows: u64, fp: u64| json!({ "dead_fp_by_tier": { "0.9": { "false_positive": fp, "rows": rows } } });
    assert_eq!(dead_fp_state(&tier(10, 1)), DeadFp::Measured(0.1));
    assert_eq!(dead_fp_state(&tier(0, 0)), DeadFp::EmptyTier);
    assert_eq!(dead_fp_state(&Value::Null), DeadFp::Unmeasured);
    let mut unlabelled = tier(10, 0);
    unlabelled["dead_unmeasured"] = json!({ "unlabelled": 2, "malformed": 0 });
    assert_eq!(dead_fp_state(&unlabelled), DeadFp::Unmeasured);
    let mut empty_but_malformed = tier(0, 0);
    empty_but_malformed["dead_unmeasured"] = json!({ "unlabelled": 0, "malformed": 1 });
    assert_eq!(dead_fp_state(&empty_but_malformed), DeadFp::Unmeasured);

    let dead_findings = |row: &Value| -> Vec<String> {
        regression_gates("go", None, row, &BTreeMap::new())
            .into_iter()
            .filter(|f| f.contains("dead false-positive rate"))
            .collect()
    };
    let empty = dead_findings(&tier(0, 0));
    assert_eq!(empty.len(), 1, "{empty:?}");
    assert!(empty[0].contains("empty tier (0 rows)"), "{empty:?}");
    assert!(!empty[0].contains("unmeasured"), "{empty:?}");
    let unmeasured = dead_findings(&Value::Null);
    assert_eq!(unmeasured.len(), 1, "{unmeasured:?}");
    assert!(unmeasured[0].contains("is unmeasured"), "{unmeasured:?}");
    assert!(dead_findings(&tier(10, 0)).is_empty());
}

/// `tools/accuracy/cmd/labelmerge` writes the file this runner reads; its
/// golden output must parse here, so the two sides cannot drift apart.
#[test]
fn the_labelmerge_golden_output_parses_as_runner_labels() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/cmd/labelmerge/testdata/merged.golden.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let labels = parse_labels(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(labels.edges.len(), 2);
    assert_eq!(labels.recall.len(), 5);
    assert_eq!(labels.oracle.len(), 1);
    assert_eq!(labels.dead.len(), 2);
    let hashed = labels
        .dead
        .iter()
        .all(|d| d.symbol_hash.as_deref().is_some_and(is_identity_hash));
    assert!(hashed, "{:?}", labels.dead);
}

fn unverifiable_pin(languages: &[&str]) -> Pin {
    Pin {
        name: "Corpus".into(),
        commit: "0".repeat(40),
        languages: languages.iter().map(|l| (*l).to_string()).collect(),
    }
}

fn committed_corpus_row() -> Value {
    json!({
        "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": {
            "go": row((99, 100), (96, 100), (100, 100), (0, 20)),
            "python": row((99, 100), (90, 100), (100, 100), (0, 20)),
        },
    })
}

#[test]
fn a_committed_row_whose_labels_are_missing_is_unverifiable_and_fails_the_gate() {
    let pin = unverifiable_pin(&["go", "python"]);
    let committed = committed_corpus_row();

    // No labels file at all: both committed rows are kept and stamped.
    let entry = carry_unverifiable(Some(&committed), None, &pin).expect("rows are carried");
    for lang in ["go", "python"] {
        let reason = entry["languages"][lang]["unverifiable"].as_str().unwrap();
        assert!(
            reason.contains("no labels file was found for Corpus"),
            "{reason}"
        );
        let found = evaluate_gates(
            lang,
            Some(&committed["languages"][lang]),
            &entry["languages"][lang],
            None,
            &gates(),
        );
        assert_eq!(found, vec![unverifiable_gate(lang, reason)], "{found:?}");
    }

    // Labels that cover only python: go is stamped, python is the fresh row.
    let fresh = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": { "python": row((99, 100), (90, 100), (100, 100), (0, 20)) } });
    let entry = carry_unverifiable(Some(&committed), Some(fresh), &pin).unwrap();
    assert!(entry["languages"]["go"]["unverifiable"]
        .as_str()
        .unwrap()
        .contains("nothing for this language"));
    assert!(entry["languages"]["python"]["unverifiable"].is_null());

    // open_gates over a record holding the stamped rows names corpus + language.
    let stamped = carry_unverifiable(Some(&committed), None, &pin).unwrap();
    let record = json!({ "corpora": [stamped], "measurements": [] });
    let open = open_gates(&record, std::slice::from_ref(&pin), &gates());
    for lang in ["go", "python"] {
        let wanted = format!("Corpus: {lang}: committed labelled row is unverifiable");
        assert!(open.iter().any(|g| g.starts_with(&wanted)), "{open:?}");
    }
}

#[test]
fn rows_with_labels_or_without_a_committed_row_are_not_marked_unverifiable() {
    let pin = unverifiable_pin(&["go", "python"]);
    let committed = committed_corpus_row();
    // Nothing committed: an unlabelled corpus stays unmeasured, not unverifiable.
    assert_eq!(carry_unverifiable(None, None, &pin), None);
    // Every committed language scored this run: the fresh entry is untouched.
    let fresh = committed_corpus_row();
    assert_eq!(
        carry_unverifiable(Some(&committed), Some(fresh.clone()), &pin),
        Some(fresh)
    );
    // A language the pin does not list is not this run's business.
    let go_only = unverifiable_pin(&["go"]);
    let fresh_go = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": { "go": row((99, 100), (96, 100), (100, 100), (0, 20)) } });
    assert_eq!(
        carry_unverifiable(Some(&committed), Some(fresh_go.clone()), &go_only),
        Some(fresh_go)
    );
    // A labels file that scored no language still stamps every committed row.
    let empty = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40), "languages": {} });
    let entry = carry_unverifiable(Some(&committed), Some(empty), &pin).unwrap();
    assert!(!entry["languages"]["go"]["unverifiable"].is_null());
}

// ------------------------------------------------------------ ATTACK probes
// Adversarial tests for the honest-miss split, tiers and unverifiable rows.
// A failing test here is a break; see the commit message of this branch.

fn attack_labels(recall: Vec<Value>, edges: Vec<Value>) -> Labels {
    parse_labels(
        &json!({ "edges": edges, "recall": recall, "dead": fixture_dead_labels() }).to_string(),
    )
    .unwrap()
}

fn attack_recall(name: &str, truth: &str) -> Value {
    json!({ "language": "go", "construct": "atk", "caller": "app/svc/svc.go::Caller01",
            "name": name, "truth": truth })
}

fn attack_honest(label: &str, labels: &Labels) -> (Value, Value) {
    let (dir, _) = go_fixture_checkout(label);
    let built = common::build(&dir);
    let scored = score_labels("Fixture", &built, labels, &["go".to_string()]).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        scored["go"]["honest_miss"].clone(),
        scored["go"]["confident_edge_precision"].clone(),
    )
}

/// Two call sites of one name from one caller: one bound HIGH and correct, the
/// other missing. `wrongly_bound` must not swallow the missing one.
#[test]
fn attack_a_correct_same_name_edge_does_not_turn_a_silent_miss_into_wrongly_bound() {
    let labels = attack_labels(
        vec![attack_recall("Target01", "app/util/util2.go::Target02")],
        vec![json!({ "language": "go", "caller": "app/svc/svc.go::Caller01",
                      "callee": "app/util/util.go::Target01", "correct": true })],
    );
    let (honest, precision) = attack_honest("atk-same-name", &labels);
    assert_eq!(precision["correct"], 1, "{precision}");
    assert_eq!(
        honest["wrongly_bound"], 0,
        "the HIGH edge is labelled CORRECT, precision never debits it, yet the missing site is hidden as wrongly_bound: {honest}"
    );
    assert_eq!(honest["silent"], 1, "{honest}");
}

/// The old matcher compared the label name whole to the ledger row's last
/// segment; the new one takes both last segments, so `zzz.X` is credited with
/// the ledger row of an unrelated receiver's `X`.
#[test]
fn attack_a_dotted_label_name_does_not_borrow_another_receivers_ledger_row() {
    let (dir, _) = go_fixture_checkout("atk-dotted");
    let built = common::build(&dir);
    let row = built
        .resolution
        .unresolved
        .iter()
        .find(|u| UNATTRIBUTED_LABELS.contains(&u.class.label()))
        .expect("an unattributed ledger row");
    let name = format!("zzz.{}", last_segment(&row.callee_name));
    let labels = parse_labels(
        &json!({ "recall": [{ "language": "go", "construct": "atk",
            "caller": row.source_symbol, "name": name, "truth": "app/util/util2.go::Target02" }],
            "dead": fixture_dead_labels() })
        .to_string(),
    )
    .unwrap();
    let scored = score_labels("Fixture", &built, &labels, &["go".to_string()]).unwrap();
    let honest = &scored["go"]["honest_miss"];
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        honest["recorded"], 0,
        "label {name:?} was credited as recorded by ledger row {:?} ({}): {honest}",
        row.callee_name, row.source_symbol
    );
}

/// A MEDIUM wrong edge is not a HIGH binding: the site stays silent/recorded.
#[test]
fn attack_a_medium_edge_is_not_wrongly_bound() {
    let (dir, _) = go_fixture_checkout("atk-medium");
    let mut built = common::build(&dir);
    // Demote every Caller01 -> *Target01 edge to MEDIUM confidence.
    let mut demoted = 0;
    for e in built.resolution.edges.iter_mut() {
        if e.source_symbol == "app/svc/svc.go::Caller01"
            && matches!(e.edge_kind, EdgeKind::Calls | EdgeKind::References)
            && last_segment(&e.target_symbol) == "Target01"
        {
            e.confidence.0 = 0.6;
            demoted += 1;
        }
    }
    assert!(demoted > 0, "no Caller01 -> Target01 edge to demote");
    let labels = attack_labels(
        vec![attack_recall("Target01", "app/util/util2.go::Target02")],
        vec![],
    );
    let scored = score_labels("Fixture", &built, &labels, &["go".to_string()]).unwrap();
    let honest = scored["go"]["honest_miss"].clone();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(honest["wrongly_bound"], 0, "{honest}");
}

/// A site with a truth edge AND a wrong same-name edge is present: no honest row.
#[test]
fn attack_a_site_with_a_truth_edge_and_a_wrong_edge_is_present() {
    let labels = attack_labels(
        vec![attack_recall(
            "Target01",
            "app/util/util.go::Target01|app/util/util2.go::Target02",
        )],
        vec![],
    );
    let (honest, _) = attack_honest("atk-both", &labels);
    assert_eq!(honest["absent"], 0, "{honest}");
    assert_eq!(honest["wrongly_bound"], 0, "{honest}");
}

/// Characterisation: with no edge label the wrong HIGH edge is in nobody's
/// precision, and the site left honest-miss. Recall (present=false) is the only
/// place it still shows.
#[test]
fn attack_an_unsampled_wrong_high_edge_leaves_precision_and_honest_miss() {
    let labels = attack_labels(
        vec![attack_recall("Target01", "app/util/util2.go::Target02")],
        vec![],
    );
    let (honest, precision) = attack_honest("atk-unsampled", &labels);
    assert_eq!(honest["wrongly_bound"], 1, "{honest}");
    assert_eq!(
        precision["total"], 1,
        "the comment says precision already debits this wrong binding; with no edge label it debits nothing: {precision}"
    );
}

#[test]
fn attack_last_segment_of_a_name_ending_in_a_separator_is_not_empty() {
    // `doThing:with:` style selectors and `a.` collapse to "" and then match
    // every other name that does the same.
    assert_ne!(last_segment("doThing:with:"), last_segment("other:"));
}

#[test]
fn attack_dead_fp_state_rejects_impossible_numbers() {
    let tier = |fp: Value, rows: Value| json!({ "dead_fp_by_tier": { "0.9": { "false_positive": fp, "rows": rows } } });
    let mut wrong = Vec::new();
    for (fp, rows) in [(json!(0), json!(-5)), (json!(-5), json!(10)), (json!(11), json!(10)), (json!(0), json!(0.5))] {
        let state = dead_fp_state(&tier(fp.clone(), rows.clone()));
        if !matches!(state, DeadFp::Unmeasured) {
            wrong.push(format!("fp={fp} rows={rows} read as {state:?}, not Unmeasured"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    // Present-but-wrong-type and missing values.
    for t in [
        tier(json!("0"), json!("10")),
        tier(json!(null), json!(10)),
        json!({ "dead_fp_by_tier": {} }),
        json!({ "dead_fp_by_tier": { "0.9": null } }),
        json!({ "dead_fp_by_tier": { "0.9": { "rows": 0 } } }),
    ] {
        assert_eq!(dead_fp_state(&t), DeadFp::Unmeasured, "{t}");
    }
}

#[test]
fn attack_a_malformed_dead_unmeasured_block_is_not_read_as_zero() {
    let mut wrong = Vec::new();
    for block in [
        json!({ "unlabelled": "2", "malformed": 0 }),
        json!({ "unlabelled": -1, "malformed": 0 }),
        json!({ "unlabelled": 2.5 }),
        json!({ "unlabelled": null }),
        json!("x"),
    ] {
        let row = json!({ "dead_fp_by_tier": { "0.9": { "false_positive": 0, "rows": 10 } },
                          "dead_unmeasured": block.clone() });
        let state = dead_fp_state(&row);
        if state != DeadFp::Unmeasured {
            wrong.push(format!("dead_unmeasured {block} read as {state:?} (the CLI reader panics on it)"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn attack_an_empty_tier_after_a_measured_one_still_fails_and_says_why() {
    let tier = |rows: u64| json!({ "dead_fp_by_tier": { "0.9": { "false_positive": 0, "rows": rows } } });
    let found = regression_gates("go", Some(&tier(10)), &tier(0), &BTreeMap::new());
    let dead: Vec<_> = found.iter().filter(|f| f.contains("dead")).collect();
    assert_eq!(dead.len(), 1, "{found:?}");
    assert!(dead[0].contains("empty tier"), "{dead:?}");
}

#[test]
fn attack_a_non_string_unverifiable_stamp_still_fails_the_gate() {
    for stamp in [json!(true), json!({}), json!(["x"]), json!(1)] {
        let mut current = row((99, 100), (96, 100), (100, 100), (0, 20));
        current["unverifiable"] = stamp.clone();
        let found = evaluate_gates("go", None, &current, None, &gates());
        assert!(
            found.iter().any(|f| f.contains("unverifiable")),
            "unverifiable={stamp} is treated as a verified row by the runner (the CLI reader treats any non-null as unverifiable): {found:?}"
        );
    }
}

#[test]
fn attack_the_old_flow_left_a_committed_row_looking_measured() {
    // Old runner: no labels -> nothing superseded -> the committed good row
    // stays and regression_gates over it is clean. New: stamped -> fails.
    let pin = unverifiable_pin(&["go"]);
    let committed = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": { "go": row((97, 100), (90, 100), (99, 100), (0, 10)) } });
    let good = &committed["languages"]["go"];
    assert!(regression_gates("go", None, good, &floors()).is_empty());
    let stamped = carry_unverifiable(Some(&committed), None, &pin).unwrap();
    let found = evaluate_gates("go", Some(good), &stamped["languages"]["go"], None, &gates());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("unverifiable"));
}

#[test]
fn attack_a_moved_pin_does_not_stamp_the_old_commit_row_under_the_new_pin() {
    let old_commit = "1".repeat(40);
    let pin = unverifiable_pin(&["go"]);
    let previous = json!({ "name": "Corpus", "corpus_commit": old_commit,
        "languages": { "go": row((97, 100), (90, 100), (99, 100), (0, 10)) } });
    let entry = carry_unverifiable(Some(&previous), None, &pin).expect("carried");
    assert_ne!(
        entry["corpus_commit"], json!(old_commit),
        "the stamped entry still claims commit {old_commit}, not the pin {}: {entry}",
        pin.commit
    );
    // With labels that cover only another language, the carried row is from a
    // different commit than the fresh entry it is spliced into.
    let fresh = json!({ "name": "Corpus", "corpus_commit": pin.commit, "languages": {} });
    let pin2 = unverifiable_pin(&["go", "python"]);
    let entry = carry_unverifiable(Some(&previous), Some(fresh), &pin2).unwrap();
    let reason = entry["languages"]["go"]["unverifiable"].as_str().unwrap();
    assert!(
        reason.contains("commit") || reason.contains("pin"),
        "row measured at {old_commit} is spliced under corpus_commit {} with reason {reason:?}",
        pin.commit
    );
}

#[test]
fn attack_double_stamping_keeps_the_first_reason_and_a_scored_run_clears_it() {
    let pin = unverifiable_pin(&["go"]);
    let committed = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": { "go": row((97, 100), (90, 100), (99, 100), (0, 10)) } });
    let once = carry_unverifiable(Some(&committed), None, &pin).unwrap();
    let twice = carry_unverifiable(Some(&once), None, &pin).unwrap();
    assert_eq!(once, twice);
    let empty_labels = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40), "languages": {} });
    let third = carry_unverifiable(Some(&once), Some(empty_labels), &pin).unwrap();
    assert_eq!(
        third["languages"]["go"]["unverifiable"], once["languages"]["go"]["unverifiable"],
        "the first reason is kept"
    );
    let fresh = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": { "go": row((98, 100), (95, 100), (99, 100), (0, 10)) } });
    let cleared = carry_unverifiable(Some(&once), Some(fresh.clone()), &pin).unwrap();
    assert_eq!(cleared, fresh);
}

#[test]
fn attack_a_pin_language_spelled_differently_from_the_committed_row_still_fails() {
    let committed = json!({ "name": "Corpus", "corpus_commit": "0".repeat(40),
        "languages": { "typescript": row((97, 100), (90, 100), (99, 100), (0, 10)) } });
    let pin = unverifiable_pin(&["ts"]);
    assert_eq!(carry_unverifiable(Some(&committed), None, &pin), None);
    // Nothing carried, nothing superseded: the run's own gate over a Null
    // current row must still fail.
    assert!(!evaluate_gates("ts", None, &Value::Null, None, &gates()).is_empty());
}

#[test]
fn attack_labels_that_are_not_an_object_of_known_sections_are_refused() {
    // `[]`, `{}` and a typo'd section name parse as "no labels" and then, at
    // best, stamp the committed row unverifiable. A typo'd section next to a
    // good one is dropped silently.
    let accepted: Vec<&str> = ["[]", "{}", r#"{"recal": []}"#, r#"{"edges": [], "recall_typo": [1]}"#]
        .into_iter()
        .filter(|raw| parse_labels(raw).is_ok())
        .collect();
    assert!(accepted.is_empty(), "parse_labels accepted {accepted:?}");
}
