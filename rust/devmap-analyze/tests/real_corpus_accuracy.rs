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
//!   `corpora.json`. A gate that held in the previous row must keep holding; a
//!   gate that did not may only not get worse, so the first record is a
//!   baseline and each language card tightens it. On top of that ratchet,
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
use devmap_analyze::{CONFIDENT_TIER_UNPROVEN, HIGHEST_DEGRADED_CONFIDENCE};
use devmap_extract::model::EdgeKind;
use devmap_resolve::UNATTRIBUTED_LABELS;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const CONFIDENT_PRECISION_FLOOR: f64 = 0.95;
const HONEST_MISS_FLOOR: f64 = 0.98;
const DEAD_FP_CEILING: f64 = 0.05;
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
    /// The true in-repo callee symbol id.
    truth: String,
}

#[derive(Debug)]
struct DeadLabel {
    language: String,
    id: String,
    truly_dead: bool,
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

fn tier_label(confidence: f32) -> &'static str {
    if f64::from(confidence) > f64::from(HIGHEST_DEGRADED_CONFIDENCE) {
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
            .entry(tier_label(r.confidence))
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
    built: &Built,
    labels: &Labels,
    languages: &[String],
) -> Result<BTreeMap<String, Value>, String> {
    let mut edge_conf: BTreeMap<(&str, &str), (i64, &'static str)> = BTreeMap::new();
    let mut any_edge: BTreeSet<(&str, &str)> = BTreeSet::new();
    for e in built
        .resolution
        .edges
        .iter()
        .filter(|e| is_use(e.edge_kind))
    {
        any_edge.insert((e.source_symbol.as_str(), e.target_symbol.as_str()));
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
    let mut dead_tier: BTreeMap<String, &'static str> = BTreeMap::new();
    for r in built.analysis.dead_symbols.iter().filter(|r| !r.is_exempt) {
        let confident = f64::from(r.confidence) > f64::from(HIGHEST_DEGRADED_CONFIDENCE)
            || r.exemption_reason
                .as_deref()
                .is_some_and(|x| x.starts_with(CONFIDENT_TIER_UNPROVEN));
        let tier = if confident { "0.9" } else { "0.4" };
        dead_tier.insert(format!("{}::{}", r.file_path, r.symbol_name), tier);
    }

    let mut out = BTreeMap::new();
    for lang in languages {
        let mut confident = Counts::default();
        let mut unique = Counts::default();
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
        let mut honest = Counts::default();
        for l in labels.recall.iter().filter(|l| &l.language == lang) {
            let present = any_edge.contains(&(l.caller.as_str(), l.truth.as_str()));
            by_construct
                .entry(l.construct.as_str())
                .or_default()
                .record(present);
            if !present {
                let recorded = ledger.get(l.caller.as_str()).is_some_and(|rows| {
                    rows.iter().any(|(name, class)| {
                        UNATTRIBUTED_LABELS.contains(class)
                            && name.rsplit(['.', ':']).next() == Some(l.name.as_str())
                    })
                });
                honest.record(recorded);
            }
        }

        let mut dead_fp: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for l in labels.dead.iter().filter(|l| &l.language == lang) {
            if let Some(tier) = dead_tier.get(&l.id) {
                let entry = dead_fp.entry(tier).or_default();
                entry.1 += 1;
                entry.0 += usize::from(!l.truly_dead);
            }
        }

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
                "honest_miss": {
                    "recorded": honest.correct,
                    "absent": honest.total,
                    "rate": honest.rate(),
                },
                "dead_fp_by_tier": dead_fp
                    .iter()
                    .map(|(tier, (fp, rows))| ((*tier).to_string(), json!({ "false_positive": fp, "rows": rows })))
                    .collect::<Map<String, Value>>(),
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

fn dead_fp_rate(row: &Value) -> Option<f64> {
    let tier = &row["dead_fp_by_tier"]["0.9"];
    let (fp, rows) = (tier["false_positive"].as_f64()?, tier["rows"].as_f64()?);
    (rows > 0.0).then(|| fp / rows)
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
/// An absolute gate that held in `previous` must hold now. One that did not
/// may only stay put (within tolerance): the first record is a baseline, and
/// each language card is what moves it. A gate whose number is `None` - no
/// labels, no sample - is reported as unmeasured, never as passed.
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
                     tolerance: f64| {
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
    );
    check(
        "honest-miss rate",
        rate_of(current, &["honest_miss", "rate"]),
        previous.and_then(|p| rate_of(p, &["honest_miss", "rate"])),
        HONEST_MISS_FLOOR,
        true,
        RECALL_TOLERANCE,
    );
    check(
        "0.9-tier dead false-positive rate",
        dead_fp_rate(current),
        previous.and_then(dead_fp_rate),
        DEAD_FP_CEILING,
        false,
        0.0,
    );
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
                check(&name, now, before, floor, true, RECALL_TOLERANCE);
            }
        }
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
    let scored = score_labels(&built, &labels, &pin.languages)?;
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

/// Put `entry` in `record[list]`. An entry already there under the same name
/// is moved to `record.superseded` (with the date it was superseded), never
/// dropped: the history of what the record said is part of the record, and
/// `corpora` also holds the labelled dead record other tests read.
fn supersede(record: &mut Value, list: &str, name: &str, entry: Value, date: &str) {
    if !record[list].is_array() {
        record[list] = json!([]);
    }
    let items = record[list].as_array_mut().expect("checked above");
    let old = items
        .iter()
        .position(|e| e["name"] == name)
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

        let previous = record["corpora"]
            .as_array()
            .and_then(|a| a.iter().find(|e| e["name"] == pin.name.as_str()))
            .cloned();
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
            "dead": [{ "language": "go", "id": "app/util/util2.go::DeadUtil", "truly_dead": true }]
        })
        .to_string(),
    )
    .unwrap();
    let scored = score_labels(&built, &labels, &pin.languages).unwrap();
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
    let err = score_labels(&built, &labels, &["go".to_string()]).unwrap_err();
    assert!(err.contains("matches no edge"), "{err}");
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
    // First record, everything below the floors: a baseline, no failure beyond
    // the unmeasured ones.
    let first = row((90, 100), (60, 100), (90, 100), (3, 10));
    assert!(regression_gates("go", None, &first, &floors()).is_empty());

    // The same numbers again: unchanged is not a regression.
    assert!(regression_gates("go", Some(&first), &first, &floors()).is_empty());

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
    let mut record = json!({ "measurements": [{ "name": "a", "v": 1 }, { "name": "b", "v": 1 }] });
    supersede(
        &mut record,
        "measurements",
        "a",
        json!({ "name": "a", "v": 2 }),
        "2026-10-13",
    );
    supersede(
        &mut record,
        "measurements",
        "c",
        json!({ "name": "c", "v": 1 }),
        "2026-10-13",
    );
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
    assert_eq!(old[0]["entry"], json!({ "name": "a", "v": 1 }));
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
