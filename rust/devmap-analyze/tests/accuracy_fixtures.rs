//! S1 — the DevMap accuracy harness: absolute gates and a known-failing ratchet
//! over the audit fixtures in `testdata/accuracy/`.
//!
//! `labelled_corpus_precision.rs` fences dead-code precision and
//! `resolution_rate_baseline.rs` fences the resolution rate against itself.
//! Neither says whether an *edge* is right, and nothing asked the question the
//! kernel's contract turns on: when the resolver binds a call at HIGH
//! confidence, is it the right symbol, and when it does not bind one, does the
//! ledger say so?
//!
//! Each fixture carries `edges.tsv`, hand-written before DevMap ran (see that
//! file's header for the columns). This test runs the product's own
//! scan -> manifests -> extract -> resolve -> analyze path (`common::build`)
//! and scores, per language:
//!
//! 1. **Precision at >= 0.9.** A `forbid` row that has a HIGH edge, or a HIGH
//!    edge from a labelled caller to a same-named symbol the row does not
//!    accept, is a *false edge*: a wrong answer with no caveat.
//! 2. **Honest miss.** An expected edge that is absent must have a ledger row
//!    in an unattributed class (`unresolved`, `uninferred_receiver`,
//!    `local_binding`) naming the call. No row, or only an *explained* class
//!    (`no_namesake`, `external`, ...) for an in-repo target, is a silent miss.
//! 3. **Dead code.** The fixture's `dead.tsv`: no confident-branch row that is
//!    not labelled truly dead, every truly-dead symbol reported, no live symbol
//!    above 0.4.
//! 4. **Recall as a ratchet.** The rows that fail today are listed in
//!    `known_failing.tsv`, each keyed by the card that owns the fix. The test
//!    fails when a listed row starts passing (tighten the ratchet), when a
//!    passing row regresses (it is not listed), and when the list or the
//!    recorded present-edge count moves the wrong way (`ratchet.json`).
//!
//! `DEVMAP_ACCURACY_BLESS=1` rewrites `known_failing.tsv` and `ratchet.json`
//! after an improvement. It refuses to write when anything got worse, and it
//! never adds a row: a new failure needs an owner and is added by hand.

mod common;

use common::{build, fixture_dirs, Built};
use devmap_analyze::{CONFIDENT_TIER_UNPROVEN, HIGHEST_DEGRADED_CONFIDENCE};
use devmap_extract::model::EdgeKind;
use devmap_resolve::UNATTRIBUTED_LABELS;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The cards that may own a known failure: the language cards, the resolver
/// seam, build manifests, dispatch, and this harness (a truth row that is
/// itself wrong). Short ids, as the board prints them.
const OWNER_CARDS: &[(&str, &str)] = &[
    ("ft-e221de3c", "L-Go"),
    ("ft-f9665721", "L-Rust"),
    ("ft-736b0b22", "L-TS"),
    ("ft-c666ba7c", "L-Python"),
    ("ft-8ca270ad", "L-Swift"),
    ("ft-0720fe1d", "L-Kotlin"),
    ("ft-32522da4", "S2 resolver"),
    ("ft-837a6f05", "S4 manifests"),
    ("ft-e3f7d1b1", "S5 dispatch"),
    ("ft-4cd88dbe", "S1 harness"),
];

/// Edge confidence in thousandths at or above which a claim is "act on it".
const HIGH_MILLIS: i64 = 900;
/// The ceiling for a symbol labelled live in `dead.tsv`.
const UNCONFIRMED_CEILING: f64 = 0.4;

// ---------------------------------------------------------------- truth rows

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Edge,
    Import,
    Forbid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Must,
    Should,
}

#[derive(Debug, Clone)]
struct Row {
    fixture: String,
    id: String,
    caller: String,
    /// Alternatives; each is a set of symbols that must all be present.
    callee: Vec<Vec<String>>,
    kind: Kind,
    level: Level,
    ledger: Vec<String>,
    language: &'static str,
}

impl Row {
    fn key(&self) -> String {
        format!("{}\t{}", self.fixture, self.id)
    }

    fn members(&self) -> impl Iterator<Item = &String> {
        self.callee.iter().flatten()
    }
}

/// The file part of a symbol id; a bare path is its own file.
fn file_of(symbol: &str) -> &str {
    symbol.split("::").next().unwrap_or(symbol)
}

/// The name a call site would write for a symbol id: the text after the last
/// `::`, then after the last `.` (`Store.PtrGet03` -> `PtrGet03`).
fn bare(symbol: &str) -> &str {
    let tail = symbol.rsplit("::").next().unwrap_or(symbol);
    tail.rsplit('.').next().unwrap_or(tail)
}

/// A call-site name reduced to its last segment (`pkg.util.helper` -> `helper`).
fn call_name(name: &str) -> &str {
    let tail = name.rsplit("::").next().unwrap_or(name);
    tail.rsplit('.').next().unwrap_or(tail)
}

fn language_of(path: &str) -> &'static str {
    match devmap_extract::languages::detect_language(Path::new(path)) {
        "tsx" | "javascript" | "jsx" => "typescript",
        "kotlin_script" => "kotlin",
        other => other,
    }
}

const SCORED: [&str; 6] = ["go", "rust", "typescript", "python", "swift", "kotlin"];

/// The language a row is scored under: the caller's, unless the caller is in a
/// language with no gate (Java calling Kotlin), then the callee's.
fn scored_language(caller: &str, callee: &[Vec<String>]) -> &'static str {
    let lang = language_of(file_of(caller));
    if SCORED.contains(&lang) {
        return lang;
    }
    callee
        .iter()
        .flatten()
        .map(|c| language_of(file_of(c)))
        .find(|l| SCORED.contains(l))
        .unwrap_or(lang)
}

fn load_rows(dir: &Path) -> Vec<Row> {
    let fixture = dir.file_name().unwrap().to_string_lossy().into_owned();
    let path = dir.join("edges.tsv");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for (n, line) in raw.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let at = format!("{}:{}", path.display(), n + 1);
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(
            cols.len() >= 6,
            "{at}: expected 8 tab-separated columns, got {line:?}"
        );
        let id = cols[0].to_string();
        assert!(seen.insert(id.clone()), "{at}: duplicate row id {id}");
        let kind = match cols[4] {
            "edge" => Kind::Edge,
            "import" => Kind::Import,
            "forbid" => Kind::Forbid,
            other => panic!("{at}: unknown kind {other:?}"),
        };
        let level = match cols[5] {
            "must" => Level::Must,
            "should" => Level::Should,
            other => panic!("{at}: unknown level {other:?}"),
        };
        let caller = cols[2].to_string();
        let callee: Vec<Vec<String>> = cols[3]
            .split('|')
            .map(|alt| alt.split('&').map(str::to_string).collect())
            .collect();
        assert!(
            callee.iter().flatten().all(|s| !s.is_empty()),
            "{at}: empty callee in {:?}",
            cols[3]
        );
        let ledger = cols
            .get(6)
            .map(|s| {
                s.split('|')
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        rows.push(Row {
            fixture: fixture.clone(),
            id,
            language: scored_language(&caller, &callee),
            caller,
            callee,
            kind,
            level,
            ledger,
        });
    }
    assert!(!rows.is_empty(), "{}: no rows", path.display());
    rows
}

// --------------------------------------------------------------- evaluation

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Ok,
    /// Absent, and the ledger says so honestly. A recall loss, not a gate failure.
    Miss,
    /// Absent with no unattributed ledger row naming it.
    SilentMiss,
    /// A HIGH edge to the wrong symbol, or to a forbidden one.
    FalseEdge,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Miss => "miss",
            Status::SilentMiss => "silent_miss",
            Status::FalseEdge => "false_edge",
        }
    }

    fn parse(s: &str) -> Option<Status> {
        [Status::Miss, Status::SilentMiss, Status::FalseEdge]
            .into_iter()
            .find(|status| status.label() == s)
    }
}

#[derive(Debug)]
struct Verdict {
    status: Status,
    detail: String,
}

/// An edge kind that answers "does the caller use the callee".
fn is_use(kind: EdgeKind) -> bool {
    !matches!(
        kind,
        EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
    )
}

struct Index<'a> {
    /// caller symbol -> use edges `(target symbol, confidence millis)`.
    uses: BTreeMap<&'a str, Vec<(&'a str, i64)>>,
    /// `(source file, target file)` -> import edge exists.
    imports: BTreeSet<(&'a str, &'a str)>,
    /// caller symbol -> unresolved rows `(callee name, class label)`.
    ledger: BTreeMap<&'a str, Vec<(&'a str, &'static str)>>,
    /// source file -> unattributed class labels seen anywhere in the file.
    file_ledger: BTreeMap<&'a str, Vec<&'static str>>,
}

fn index(built: &Built) -> Index<'_> {
    let mut idx = Index {
        uses: BTreeMap::new(),
        imports: BTreeSet::new(),
        ledger: BTreeMap::new(),
        file_ledger: BTreeMap::new(),
    };
    for edge in &built.resolution.edges {
        if edge.edge_kind == EdgeKind::Imports {
            idx.imports
                .insert((edge.source_file.as_str(), edge.target_file.as_str()));
        } else if is_use(edge.edge_kind) {
            idx.uses
                .entry(edge.source_symbol.as_str())
                .or_default()
                .push((edge.target_symbol.as_str(), edge.confidence.to_millis()));
        }
    }
    for u in &built.resolution.unresolved {
        let label = u.class.label();
        idx.ledger
            .entry(u.source_symbol.as_str())
            .or_default()
            .push((u.callee_name.as_str(), label));
        idx.file_ledger
            .entry(u.source_file.as_str())
            .or_default()
            .push(label);
    }
    idx
}

fn evaluate(rows: &[Row], idx: &Index) -> Vec<Verdict> {
    // What each caller may legitimately bind a given bare name to: the union of
    // every positive row's accepted symbols. A HIGH edge to a same-named symbol
    // outside it is the namesake false edge.
    let mut accepted: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.kind == Kind::Edge) {
        for member in row.members() {
            accepted
                .entry((row.caller.as_str(), bare(member)))
                .or_default()
                .insert(member.as_str());
        }
    }
    let empty: Vec<(&str, i64)> = Vec::new();

    rows.iter()
        .map(|row| {
            let uses = idx.uses.get(row.caller.as_str()).unwrap_or(&empty);
            let high = |target: &str| {
                uses.iter()
                    .any(|(t, millis)| *t == target && *millis >= HIGH_MILLIS)
            };
            let any = |target: &str| uses.iter().any(|(t, _)| *t == target);
            match row.kind {
                Kind::Forbid => {
                    let hit: Vec<&String> = row.members().filter(|m| high(m)).collect();
                    if hit.is_empty() {
                        Verdict {
                            status: Status::Ok,
                            detail: String::new(),
                        }
                    } else {
                        Verdict {
                            status: Status::FalseEdge,
                            detail: format!("forbidden HIGH edge to {hit:?}"),
                        }
                    }
                }
                Kind::Import => {
                    let from = file_of(&row.caller);
                    let present = row.callee.iter().any(|alt| {
                        alt.iter()
                            .all(|to| idx.imports.contains(&(from, file_of(to))))
                    });
                    if present {
                        Verdict {
                            status: Status::Ok,
                            detail: String::new(),
                        }
                    } else {
                        // An import has no call-site name, so the honest record
                        // is any unattributed ledger row in the importing file.
                        let recorded = idx.file_ledger.get(from).is_some_and(|labels| {
                            labels.iter().any(|l| UNATTRIBUTED_LABELS.contains(l))
                        });
                        miss_verdict(
                            recorded,
                            "no unattributed ledger row in the importing file".into(),
                        )
                    }
                }
                Kind::Edge => {
                    let wrong: Vec<String> = uses
                        .iter()
                        .filter(|(target, millis)| {
                            *millis >= HIGH_MILLIS
                                && accepted
                                    .get(&(row.caller.as_str(), bare(target)))
                                    .is_some_and(|ok| !ok.contains(target))
                        })
                        .map(|(target, _)| (*target).to_string())
                        .collect();
                    if !wrong.is_empty() {
                        return Verdict {
                            status: Status::FalseEdge,
                            detail: format!("HIGH edge to the wrong namesake {wrong:?}"),
                        };
                    }
                    let present = row.callee.iter().any(|alt| alt.iter().all(|to| any(to)));
                    if present {
                        return Verdict {
                            status: Status::Ok,
                            detail: String::new(),
                        };
                    }
                    let mut wanted: BTreeSet<&str> =
                        row.ledger.iter().map(String::as_str).collect();
                    wanted.extend(row.members().map(|m| bare(m)));
                    let rows = idx.ledger.get(row.caller.as_str());
                    let seen: Vec<(&str, &str)> = rows
                        .into_iter()
                        .flatten()
                        .filter(|(name, _)| wanted.contains(call_name(name)))
                        .copied()
                        .collect();
                    let recorded = seen
                        .iter()
                        .any(|(_, label)| UNATTRIBUTED_LABELS.contains(label));
                    miss_verdict(
                        recorded,
                        if seen.is_empty() {
                            format!("no ledger row names {wanted:?}")
                        } else {
                            format!("ledger rows exist but only in explained classes: {seen:?}")
                        },
                    )
                }
            }
        })
        .collect()
}

fn miss_verdict(recorded: bool, why: String) -> Verdict {
    if recorded {
        Verdict {
            status: Status::Miss,
            detail: String::new(),
        }
    } else {
        Verdict {
            status: Status::SilentMiss,
            detail: why,
        }
    }
}

// ------------------------------------------------------------ known failing

#[derive(Debug, Clone)]
struct Known {
    status: Status,
    card: String,
    note: String,
}

fn known_path() -> PathBuf {
    common::accuracy_root().join("known_failing.tsv")
}

fn ratchet_path() -> PathBuf {
    common::accuracy_root().join("ratchet.json")
}

/// `fixture\tid -> Known`.
fn load_known() -> BTreeMap<String, Known> {
    let raw = std::fs::read_to_string(known_path())
        .unwrap_or_else(|e| panic!("{}: {e}", known_path().display()));
    let mut out = BTreeMap::new();
    for (n, line) in raw.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let at = format!("known_failing.tsv:{}", n + 1);
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(
            cols.len() >= 4,
            "{at}: expected fixture, id, status, card, note: {line:?}"
        );
        let status =
            Status::parse(cols[2]).unwrap_or_else(|| panic!("{at}: unknown status {:?}", cols[2]));
        assert!(
            OWNER_CARDS.iter().any(|(card, _)| *card == cols[3]),
            "{at}: {:?} is not an owning card; known failures are keyed by the card that fixes them ({:?})",
            cols[3],
            OWNER_CARDS.iter().map(|(c, _)| *c).collect::<Vec<_>>()
        );
        let key = format!("{}\t{}", cols[0], cols[1]);
        let entry = Known {
            status,
            card: cols[3].to_string(),
            note: cols.get(4).copied().unwrap_or("").to_string(),
        };
        assert!(out.insert(key, entry).is_none(), "{at}: duplicate entry");
    }
    out
}

fn write_known(known: &BTreeMap<String, Known>) {
    let mut text = String::from(
        "# Rows that fail today, each keyed by the card that owns the fix.\n\
         # fixture<TAB>row id<TAB>status<TAB>card<TAB>note\n\
         # status: miss (absent, honest ledger row), silent_miss (absent, no unattributed ledger row),\n\
         #         false_edge (a HIGH edge to the wrong symbol or a forbidden one).\n\
         # The set only shrinks: accuracy_fixtures.rs fails when a listed row starts passing and when\n\
         # a passing row is not listed. Fixing a row means deleting its line (DEVMAP_ACCURACY_BLESS=1\n\
         # does it); adding a line needs an owning card and a reason.\n",
    );
    for (key, entry) in known {
        text.push_str(&format!(
            "{key}\t{}\t{}\t{}\n",
            entry.status.label(),
            entry.card,
            entry.note
        ));
    }
    std::fs::write(known_path(), text).expect("write known_failing.tsv");
}

// ------------------------------------------------------------------ scoring

#[derive(Default, Debug, Clone)]
struct LangScore {
    positive_rows: usize,
    must_rows: usize,
    should_rows: usize,
    forbid_rows: usize,
    present: usize,
    miss: usize,
    silent_miss: usize,
    false_edge_rows: usize,
    /// HIGH edges that are right / wrong, over the scored (caller, bare name) pairs.
    high_correct: usize,
    high_false: usize,
    dead_truly_dead: usize,
    dead_found: usize,
    dead_confident_rows: usize,
    dead_confident_false: usize,
}

impl LangScore {
    fn recall(&self) -> f64 {
        ratio(self.present, self.positive_rows)
    }
    fn precision(&self) -> f64 {
        ratio(self.high_correct, self.high_correct + self.high_false)
    }
    fn honest_miss(&self) -> f64 {
        ratio(self.miss, self.miss + self.silent_miss)
    }
}

fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        1.0
    } else {
        n as f64 / d as f64
    }
}

/// HIGH edges from the labelled callers to either an accepted symbol (right) or
/// a wrong same-named / forbidden one (wrong), counted once per pair.
fn high_edge_counts(rows: &[Row], idx: &Index, lang: &str) -> (usize, usize) {
    let mut good: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut bad: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut accepted: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|r| r.kind == Kind::Edge && r.language == lang)
    {
        for m in row.members() {
            accepted
                .entry((row.caller.as_str(), bare(m)))
                .or_default()
                .insert(m.as_str());
        }
    }
    for row in rows.iter().filter(|r| r.language == lang) {
        let Some(uses) = idx.uses.get(row.caller.as_str()) else {
            continue;
        };
        for (target, _) in uses.iter().filter(|(_, m)| *m >= HIGH_MILLIS) {
            let target = *target;
            match row.kind {
                Kind::Forbid if row.members().any(|m| m == target) => {
                    bad.insert((row.caller.as_str(), target));
                }
                Kind::Edge => {
                    if let Some(ok) = accepted.get(&(row.caller.as_str(), bare(target))) {
                        if ok.contains(target) {
                            good.insert((row.caller.as_str(), target));
                        } else {
                            bad.insert((row.caller.as_str(), target));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (good.len(), bad.len())
}

// ---------------------------------------------------------------- dead code

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeadVerdict {
    TrulyDead,
    Live,
    ExportedUnreferenced,
    Exempt,
}

fn load_dead(dir: &Path) -> Option<BTreeMap<String, DeadVerdict>> {
    let raw = std::fs::read_to_string(dir.join("dead.tsv")).ok()?;
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let verdict = match cols.next() {
            Some("truly_dead") => DeadVerdict::TrulyDead,
            Some("live") => DeadVerdict::Live,
            Some("exported_unreferenced") => DeadVerdict::ExportedUnreferenced,
            Some("exempt") => DeadVerdict::Exempt,
            other => panic!("{}: unknown dead verdict {other:?}", dir.display()),
        };
        let id = cols
            .next()
            .unwrap_or_else(|| panic!("{}: no id in {line:?}", dir.display()));
        out.insert(id.to_string(), verdict);
    }
    Some(out)
}

/// Dead-code violations for one fixture, counted into `score` by language.
fn check_dead(
    fixture: &str,
    truth: &BTreeMap<String, DeadVerdict>,
    built: &Built,
    scores: &mut BTreeMap<&'static str, LangScore>,
    violations: &mut Vec<String>,
) {
    let mut reported: BTreeMap<String, f32> = BTreeMap::new();
    for report in built.analysis.dead_symbols.iter().filter(|r| !r.is_exempt) {
        let id = format!("{}::{}", report.file_path, report.symbol_name);
        let confident = f64::from(report.confidence) > f64::from(HIGHEST_DEGRADED_CONFIDENCE)
            || report
                .exemption_reason
                .as_deref()
                .is_some_and(|r| r.starts_with(CONFIDENT_TIER_UNPROVEN));
        if confident {
            let score = scores.entry(language_of(&report.file_path)).or_default();
            score.dead_confident_rows += 1;
            if truth.get(&id) != Some(&DeadVerdict::TrulyDead) {
                score.dead_confident_false += 1;
                violations.push(format!(
                    "{fixture}: {id} is a 0.9-tier dead row ({:.2}) not labelled truly dead",
                    report.confidence
                ));
            }
        }
        reported.insert(id, report.confidence);
    }
    for cluster in &built.analysis.dead_clusters.clusters {
        for member in &cluster.members {
            reported.entry(member.clone()).or_insert(cluster.confidence);
        }
    }
    for (id, verdict) in truth {
        let lang = language_of(file_of(id));
        match verdict {
            DeadVerdict::TrulyDead => {
                let score = scores.entry(lang).or_default();
                score.dead_truly_dead += 1;
                if reported.contains_key(id) {
                    score.dead_found += 1;
                } else {
                    violations.push(format!("{fixture}: truly dead {id} is not reported"));
                }
            }
            DeadVerdict::Live => {
                if let Some(c) = reported
                    .get(id)
                    .filter(|c| f64::from(**c) > UNCONFIRMED_CEILING + 1e-6)
                {
                    violations.push(format!("{fixture}: live {id} reported above 0.4 ({c:.2})"));
                }
            }
            DeadVerdict::Exempt => {
                if reported.contains_key(id) {
                    violations.push(format!("{fixture}: exempt {id} must never be a candidate"));
                }
            }
            DeadVerdict::ExportedUnreferenced => {}
        }
    }
}

// --------------------------------------------------------------------- run

struct Run {
    rows: Vec<Row>,
    verdicts: Vec<Verdict>,
    scores: BTreeMap<&'static str, LangScore>,
    dead_violations: Vec<String>,
}

fn run() -> Run {
    let mut rows = Vec::new();
    let mut verdicts = Vec::new();
    let mut scores: BTreeMap<&'static str, LangScore> = BTreeMap::new();
    let mut dead_violations = Vec::new();

    for dir in fixture_dirs() {
        let built = build(&dir);
        let fixture_rows = load_rows(&dir);
        let idx = index(&built);
        let fixture_verdicts = evaluate(&fixture_rows, &idx);

        let langs: BTreeSet<&'static str> = fixture_rows.iter().map(|r| r.language).collect();
        for lang in langs {
            let (good, bad) = high_edge_counts(&fixture_rows, &idx, lang);
            let score = scores.entry(lang).or_default();
            score.high_correct += good;
            score.high_false += bad;
        }
        for (row, verdict) in fixture_rows.iter().zip(&fixture_verdicts) {
            let score = scores.entry(row.language).or_default();
            match row.kind {
                Kind::Forbid => score.forbid_rows += 1,
                _ => {
                    score.positive_rows += 1;
                    match row.level {
                        Level::Must => score.must_rows += 1,
                        Level::Should => score.should_rows += 1,
                    }
                    if verdict.status == Status::Ok {
                        score.present += 1;
                    }
                }
            }
            match verdict.status {
                Status::Miss => score.miss += 1,
                Status::SilentMiss => score.silent_miss += 1,
                Status::FalseEdge => score.false_edge_rows += 1,
                Status::Ok => {}
            }
        }
        if let Some(truth) = load_dead(&dir) {
            check_dead(
                dir.file_name().unwrap().to_str().unwrap(),
                &truth,
                &built,
                &mut scores,
                &mut dead_violations,
            );
        }
        rows.extend(fixture_rows);
        verdicts.extend(fixture_verdicts);
    }
    Run {
        rows,
        verdicts,
        scores,
        dead_violations,
    }
}

fn report(run: &Run) -> String {
    let mut out = String::from(
        "\nlanguage    rows(must/should/forbid)  recall  precision@0.9  honest-miss  dead(found/truly, 0.9-FP)\n",
    );
    for (lang, s) in &run.scores {
        out.push_str(&format!(
            "{lang:11} {:3}/{:3}/{:3}  {:>8}/{:<3} {:6.3}  {:6.3} ({:2}/{:2})  {:6.3} ({}/{})  {}/{}, {}/{}\n",
            s.must_rows,
            s.should_rows,
            s.forbid_rows,
            s.present,
            s.positive_rows,
            s.recall(),
            s.precision(),
            s.high_correct,
            s.high_correct + s.high_false,
            s.honest_miss(),
            s.miss,
            s.miss + s.silent_miss,
            s.dead_found,
            s.dead_truly_dead,
            s.dead_confident_false,
            s.dead_confident_rows,
        ));
    }
    out
}

// ----------------------------------------------------------------- ratchet

/// `{ "<language>": { "positive_rows", "present", "known_failing" } }`.
fn ratchet_of(run: &Run, known: &BTreeMap<String, Known>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (lang, s) in &run.scores {
        let listed = run
            .rows
            .iter()
            .filter(|r| r.language == *lang && known.contains_key(&r.key()))
            .count();
        map.insert(
            (*lang).to_string(),
            serde_json::json!({
                "positive_rows": s.positive_rows,
                "present": s.present,
                "known_failing": listed,
            }),
        );
    }
    serde_json::Value::Object(map)
}

/// The per-language fixture measurement, committed in `record.json` under
/// `fixtures` so the numbers a language card must move are in the record beside
/// the corpus rows. Read-modify-write: `corpora` and `measurements` belong to
/// `real_corpus_accuracy.rs` and are left untouched.
fn write_fixture_record(run: &Run) {
    let path = common::accuracy_root().join("record.json");
    let mut record: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({ "schema": 1, "corpora": [], "measurements": [] }));
    let commit = std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string());
    let mut fixtures = serde_json::Map::new();
    for (lang, s) in &run.scores {
        fixtures.insert(
            (*lang).to_string(),
            serde_json::json!({
                "devmap_commit": commit,
                "expected_edge_rows": s.positive_rows,
                "present": s.present,
                "recall": round3(s.recall()),
                "forbid_rows": s.forbid_rows,
                "high_edges_correct": s.high_correct,
                "high_edges_false": s.high_false,
                "precision_at_0.9": round3(s.precision()),
                "honest_miss": { "recorded": s.miss, "absent": s.miss + s.silent_miss, "rate": round3(s.honest_miss()) },
                "dead": {
                    "truly_dead": s.dead_truly_dead,
                    "found": s.dead_found,
                    "confident_rows": s.dead_confident_rows,
                    "confident_false": s.dead_confident_false,
                },
            }),
        );
    }
    record["fixtures"] = serde_json::Value::Object(fixtures);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&record).expect("record serializes") + "\n",
    )
    .expect("write record.json");
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// What comparing the measurement with `known_failing.tsv` found.
struct Diff {
    /// Failures that are not (or are no longer) allowed: a new failing row, a
    /// listed row that got worse, a listed row that has no truth row.
    regressions: Vec<String>,
    /// Improvements the committed list has not caught up with.
    tighten: Vec<String>,
    /// `known` with the improvements applied.
    next_known: BTreeMap<String, Known>,
}

/// The ratchet. `observed` is `(key, status, detail)` for every truth row.
fn diff_against_known(
    observed: &[(String, Status, String)],
    known: &BTreeMap<String, Known>,
) -> Diff {
    let mut diff = Diff {
        regressions: Vec::new(),
        tighten: Vec::new(),
        next_known: known.clone(),
    };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (key, status, detail) in observed {
        seen.insert(key.as_str());
        match (*status, known.get(key)) {
            (Status::Ok, None) => {}
            (Status::Ok, Some(k)) => {
                diff.tighten.push(format!(
                    "{key} now passes but is still in known_failing.tsv as {} ({}) - remove it",
                    k.status.label(),
                    k.card
                ));
                diff.next_known.remove(key);
            }
            (status, None) => diff.regressions.push(format!(
                "{key} is {} and is not a known failure: {detail} [{}]",
                status.label(),
                gate_name(status)
            )),
            (status, Some(k)) if status > k.status => diff.regressions.push(format!(
                "{key} got worse: {} -> {}: {detail} [{}]",
                k.status.label(),
                status.label(),
                gate_name(status)
            )),
            (status, Some(k)) if status < k.status => {
                diff.tighten.push(format!(
                    "{key} improved from {} to {} - update known_failing.tsv",
                    k.status.label(),
                    status.label()
                ));
                if let Some(entry) = diff.next_known.get_mut(key) {
                    entry.status = status;
                }
            }
            _ => {}
        }
    }
    for key in known.keys().filter(|k| !seen.contains(k.as_str())) {
        diff.regressions.push(format!(
            "{key} is in known_failing.tsv but no edges.tsv row has that id"
        ));
    }
    diff
}

#[test]
fn the_fixtures_meet_the_gates_and_the_ratchet_only_tightens() {
    let run = run();
    println!("{}", report(&run));
    if let Some(path) = std::env::var_os("DEVMAP_ACCURACY_DUMP") {
        // Bootstrap for a new fixture: every non-passing row, so an owner can be
        // assigned by hand. Not a mode CI uses.
        let mut text = String::new();
        for (row, verdict) in run.rows.iter().zip(&run.verdicts) {
            if verdict.status != Status::Ok {
                text.push_str(&format!(
                    "{}\t{}\t{}\n",
                    row.key(),
                    verdict.status.label(),
                    verdict.detail
                ));
            }
        }
        std::fs::write(path, text).expect("write dump");
        return;
    }
    let known = load_known();
    let bless = std::env::var_os("DEVMAP_ACCURACY_BLESS").is_some();

    let observed: Vec<(String, Status, String)> = run
        .rows
        .iter()
        .zip(&run.verdicts)
        .map(|(row, verdict)| (row.key(), verdict.status, verdict.detail.clone()))
        .collect();
    let Diff {
        regressions,
        tighten,
        next_known,
    } = diff_against_known(&observed, &known);

    if bless {
        assert!(
            regressions.is_empty(),
            "refusing to bless a worse state:\n  {}",
            regressions.join("\n  ")
        );
        write_known(&next_known);
        let ratchet = ratchet_of(&run, &next_known);
        std::fs::write(
            ratchet_path(),
            serde_json::to_string_pretty(&ratchet).expect("ratchet serializes") + "\n",
        )
        .expect("write ratchet.json");
        write_fixture_record(&run);
        eprintln!("known_failing.tsv and ratchet.json rewritten from the current measurement");
        return;
    }

    assert!(
        regressions.is_empty(),
        "accuracy regressed (precision, honest-miss and recall gates):\n  {}",
        regressions.join("\n  ")
    );
    assert!(
        tighten.is_empty(),
        "the ratchet must be tightened — a known failure moved the right way. Run \
         DEVMAP_ACCURACY_BLESS=1 cargo test -p devmap-analyze --test accuracy_fixtures and commit:\n  {}",
        tighten.join("\n  ")
    );
    assert!(
        !known.is_empty() || run.verdicts.iter().all(|v| v.status == Status::Ok),
        "known_failing.tsv is empty although rows fail"
    );
    assert!(
        run.dead_violations.is_empty(),
        "dead-code gates failed:\n  {}",
        run.dead_violations.join("\n  ")
    );

    // The recorded counts only move one way. Row-level equality above already
    // pins today's state; this stops a hand edit to known_failing.tsv or a
    // rewritten ratchet from admitting a worse one.
    let recorded: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ratchet_path()).expect("ratchet.json is readable"),
    )
    .expect("ratchet.json is JSON");
    let now = ratchet_of(&run, &known);
    let mut moved = Vec::new();
    for (lang, current) in now.as_object().unwrap() {
        let Some(old) = recorded.get(lang) else {
            moved.push(format!("{lang}: not in ratchet.json"));
            continue;
        };
        if current["present"].as_u64() < old["present"].as_u64() {
            moved.push(format!(
                "{lang}: present edges fell {} -> {}",
                old["present"], current["present"]
            ));
        }
        if current["known_failing"].as_u64() > old["known_failing"].as_u64() {
            moved.push(format!(
                "{lang}: known_failing grew {} -> {}",
                old["known_failing"], current["known_failing"]
            ));
        }
        if current["positive_rows"].as_u64() < old["positive_rows"].as_u64() {
            moved.push(format!(
                "{lang}: expected rows were deleted {} -> {}",
                old["positive_rows"], current["positive_rows"]
            ));
        }
    }
    assert!(
        moved.is_empty(),
        "ratchet.json says the counts may only improve:\n  {}",
        moved.join("\n  ")
    );
}

fn gate_name(status: Status) -> &'static str {
    match status {
        Status::FalseEdge => "gate: precision at >= 0.9",
        Status::SilentMiss => "gate: honest miss",
        Status::Miss => "ratchet: expected edge recall",
        Status::Ok => "",
    }
}

#[test]
fn a_known_failure_is_always_keyed_by_an_owning_card_and_has_a_reason() {
    for (key, entry) in load_known() {
        assert!(
            !entry.note.trim().is_empty(),
            "{key}: a known failure carries the reason it fails today"
        );
        assert!(
            OWNER_CARDS.iter().any(|(c, _)| *c == entry.card),
            "{key}: unknown card {}",
            entry.card
        );
    }
}

#[test]
fn every_fixture_row_names_a_language_the_harness_scores() {
    let scored = SCORED;
    for dir in fixture_dirs() {
        for row in load_rows(&dir) {
            assert!(
                scored.contains(&row.language),
                "{}: row {} caller {} is in {:?}, which has no gate",
                row.fixture,
                row.id,
                row.caller,
                row.language
            );
        }
    }
    let all: BTreeSet<&str> = fixture_dirs()
        .iter()
        .flat_map(|d| load_rows(d))
        .map(|r| r.language)
        .collect();
    for lang in scored {
        assert!(all.contains(lang), "no fixture row exercises {lang}");
    }
}

fn known_entry(status: Status) -> Known {
    Known {
        status,
        card: "ft-e221de3c".into(),
        note: "synthetic".into(),
    }
}

fn obs(key: &str, status: Status) -> (String, Status, String) {
    (key.to_string(), status, "synthetic".into())
}

#[test]
fn the_ratchet_fails_when_a_known_failure_starts_passing() {
    let known = BTreeMap::from([("go\tgo.C03".to_string(), known_entry(Status::Miss))]);
    let diff = diff_against_known(&[obs("go\tgo.C03", Status::Ok)], &known);
    assert!(diff.regressions.is_empty());
    assert_eq!(diff.tighten.len(), 1, "{:?}", diff.tighten);
    assert!(diff.tighten[0].contains("now passes"));
    assert!(
        diff.next_known.is_empty(),
        "the blessed list drops the fixed row"
    );
}

#[test]
fn the_ratchet_fails_when_a_passing_row_regresses() {
    let diff = diff_against_known(&[obs("go\tgo.C01", Status::SilentMiss)], &BTreeMap::new());
    assert_eq!(diff.regressions.len(), 1, "{:?}", diff.regressions);
    assert!(diff.regressions[0].contains("gate: honest miss"));
    let diff = diff_against_known(&[obs("go\tgo.C01", Status::FalseEdge)], &BTreeMap::new());
    assert!(diff.regressions[0].contains("gate: precision"));
    let diff = diff_against_known(&[obs("go\tgo.C01", Status::Miss)], &BTreeMap::new());
    assert!(diff.regressions[0].contains("ratchet: expected edge recall"));
}

#[test]
fn the_ratchet_fails_when_a_known_failure_gets_worse_and_accepts_an_improvement_only_as_a_tighten()
{
    let known = BTreeMap::from([("go\tgo.C03".to_string(), known_entry(Status::Miss))]);
    let worse = diff_against_known(&[obs("go\tgo.C03", Status::FalseEdge)], &known);
    assert_eq!(worse.regressions.len(), 1);
    let known = BTreeMap::from([("go\tgo.C03".to_string(), known_entry(Status::FalseEdge))]);
    let better = diff_against_known(&[obs("go\tgo.C03", Status::Miss)], &known);
    assert!(better.regressions.is_empty());
    assert_eq!(better.tighten.len(), 1);
    assert_eq!(better.next_known["go\tgo.C03"].status, Status::Miss);
}

#[test]
fn a_known_failure_without_a_truth_row_is_stale() {
    let known = BTreeMap::from([("go\tgo.GONE".to_string(), known_entry(Status::Miss))]);
    let diff = diff_against_known(&[], &known);
    assert_eq!(diff.regressions.len(), 1);
}

#[test]
fn an_unchanged_state_is_clean() {
    let known = BTreeMap::from([("go\tgo.C03".to_string(), known_entry(Status::Miss))]);
    let diff = diff_against_known(
        &[
            obs("go\tgo.C03", Status::Miss),
            obs("go\tgo.C01", Status::Ok),
        ],
        &known,
    );
    assert!(diff.regressions.is_empty() && diff.tighten.is_empty());
}
