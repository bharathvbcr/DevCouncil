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
//!   baseline and each language card tightens it.
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

fn load_pins() -> (Vec<Pin>, BTreeMap<String, f64>) {
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
    let floors = value["recall_floors_percent"]
        .as_object()
        .expect("recall_floors_percent")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_f64().expect("floor") / 100.0))
        .collect();
    (pins, floors)
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

/// The recall to hold against `floor`: Rust is gated on its two classes
/// separately, every other language on the overall rate.
fn recall_for_floor(
    lang: &str,
    row: &Value,
    floors: &BTreeMap<String, f64>,
) -> Vec<(String, Option<f64>, f64)> {
    if lang == "rust" {
        ["free_self", "non_self"]
            .iter()
            .filter_map(|class| {
                let floor = *floors.get(&format!("rust_{class}"))?;
                let c = &row["recall"]["by_construct"][*class];
                let rate = c["rate"].as_f64();
                Some((format!("recall[{class}]"), rate, floor))
            })
            .collect()
    } else {
        floors
            .get(lang)
            .map(|floor| {
                vec![(
                    "recall".to_string(),
                    rate_of(row, &["recall", "rate"]),
                    *floor,
                )]
            })
            .unwrap_or_default()
    }
}

/// Gate violations for one language row.
///
/// An absolute gate that held in `previous` must hold now. One that did not
/// may only stay put (within tolerance): the first record is a baseline, and
/// each language card is what moves it. A gate whose number is `None` - no
/// labels, no sample - is reported as unmeasured, never as passed.
fn evaluate_gates(
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
    for (name, now, floor) in recall_for_floor(lang, current, floors) {
        let before = previous.and_then(|p| {
            recall_for_floor(lang, p, floors)
                .into_iter()
                .find(|(n, _, _)| *n == name)
                .and_then(|(_, r, _)| r)
        });
        check(&name, now, before, floor, true, RECALL_TOLERANCE);
    }
    out
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

/// Measure one pinned corpus. Returns its `measurements` entry, and its
/// `corpora` entry when it has labels.
fn measure_corpus(dir: &Path, pin: &Pin) -> Result<(Value, Option<Value>), String> {
    verify_pin(dir, pin)?;
    let built = common::build(dir);
    let structure = structure(&built, &pin.languages);
    let date = today();
    let commit = devmap_commit();
    let measurement = json!({
        "name": pin.name,
        "date": date,
        "devmap_commit": commit,
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

/// Replace the entry named `name` in `list`, or append it.
fn upsert(list: &mut Value, name: &str, entry: Value) {
    let items = list.as_array_mut().expect("record list is an array");
    match items.iter_mut().find(|e| e["name"] == name) {
        Some(slot) => *slot = entry,
        None => items.push(entry),
    }
}

#[test]
#[ignore = "needs DEVMAP_CORPUS_<NAME> checkouts of the owner's private repositories"]
fn score_the_pinned_corpora_and_write_the_record() {
    let (pins, floors) = load_pins();
    let mut record = load_record();
    let mut failures = Vec::new();
    let mut ran = 0;

    for pin in &pins {
        let Some(dir) = std::env::var_os(env_name(&pin.name)).map(PathBuf::from) else {
            eprintln!("skipping {}: {} is not set", pin.name, env_name(&pin.name));
            continue;
        };
        ran += 1;
        let (measurement, labelled) =
            measure_corpus(&dir, pin).unwrap_or_else(|e| panic!("{}: {e}", pin.name));
        println!("{}", serde_json::to_string_pretty(&measurement).unwrap());
        upsert(&mut record["measurements"], &pin.name, measurement);

        if let Some(entry) = labelled {
            let previous = record["corpora"]
                .as_array()
                .and_then(|a| a.iter().find(|e| e["name"] == pin.name.as_str()))
                .cloned();
            for lang in &pin.languages {
                let prev_row = previous
                    .as_ref()
                    .map(|p| &p["languages"][lang.as_str()])
                    .filter(|v| !v.is_null());
                let current = &entry["languages"][lang.as_str()];
                if current.is_null() {
                    continue;
                }
                failures.extend(
                    evaluate_gates(lang, prev_row, current, &floors)
                        .into_iter()
                        .map(|f| format!("{}: {f}", pin.name)),
                );
            }
            upsert(&mut record["corpora"], &pin.name, entry);
        }
    }
    assert!(
        ran > 0,
        "no DEVMAP_CORPUS_<NAME> variable is set; nothing was measured"
    );

    // The record is written before the gates are asserted: a failing gate is a
    // finding to commit and explain, not a number to hide.
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
    assert_eq!(
        go["recall"]["present"], 1,
        "Caller03 is a known miss today: {go}"
    );
    assert_eq!(go["honest_miss"]["absent"], 1);
    assert_eq!(
        go["honest_miss"]["recorded"], 1,
        "the miss is an uninferred_receiver ledger row: {go}"
    );
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
    load_pins().1
}

#[test]
fn a_gate_that_held_must_keep_holding_and_one_that_did_not_may_only_stay_put() {
    // First record, everything below the floors: a baseline, no failure beyond
    // the unmeasured ones.
    let first = row((90, 100), (60, 100), (90, 100), (3, 10));
    assert!(evaluate_gates("go", None, &first, &floors()).is_empty());

    // The same numbers again: unchanged is not a regression.
    assert!(evaluate_gates("go", Some(&first), &first, &floors()).is_empty());

    // Precision fell two points: a regression even though it never met 0.95.
    let worse = row((88, 100), (60, 100), (90, 100), (3, 10));
    let found = evaluate_gates("go", Some(&first), &worse, &floors());
    assert!(
        found
            .iter()
            .any(|f| f.contains("confident edge precision regressed")),
        "{found:?}"
    );

    // Met the floor once, must meet it again.
    let good = row((97, 100), (90, 100), (99, 100), (0, 10));
    assert!(evaluate_gates("go", Some(&good), &good, &floors()).is_empty());
    let slipped = row((94, 100), (90, 100), (99, 100), (0, 10));
    let found = evaluate_gates("go", Some(&good), &slipped, &floors());
    assert!(
        found.iter().any(|f| f.contains("no longer meets")),
        "{found:?}"
    );

    // Recall under the Go floor (88) that previously held.
    let low_recall = row((97, 100), (80, 100), (99, 100), (0, 10));
    let found = evaluate_gates("go", Some(&good), &low_recall, &floors());
    assert!(found.iter().any(|f| f.contains("recall")), "{found:?}");

    // Dead false positives rising is a regression with zero tolerance.
    let dead_worse = row((97, 100), (90, 100), (99, 100), (1, 10));
    let found = evaluate_gates("go", Some(&good), &dead_worse, &floors());
    assert!(
        found.iter().any(|f| f.contains("dead false-positive")),
        "{found:?}"
    );
}

#[test]
fn an_unmeasured_gate_is_reported_and_never_passes() {
    let unlabelled = row((0, 0), (0, 0), (0, 0), (0, 0));
    let found = evaluate_gates("python", None, &unlabelled, &floors());
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
    let found = evaluate_gates("rust", Some(&prev), &r, &floors());
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
    let (pins, floors) = load_pins();
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
