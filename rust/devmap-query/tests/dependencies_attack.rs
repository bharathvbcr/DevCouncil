//! Adversarial probes for the outbound `dependencies` contract
//! (`lane/deps-outbound` @ 55a1c302). Failing tests here are findings: each
//! doc comment names the probe, the claim it attacks, and the severity if it
//! breaks. Nothing here fixes anything.
//!
//! Every probe drives both engines on one fixture, as `dependencies_are_outbound`
//! does, and compares them on the whole envelope, not just the items.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{QueryEngine, Request, ResolutionAvailability, Response, StoreQueryEngine};
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

struct Fixture {
    extractions: Vec<Extraction>,
    resolution: ResolutionResult,
    store: Store,
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    Fixture {
        extractions,
        resolution,
        store,
    }
}

fn request(query: &str, budget: u32) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: budget,
        min_confidence: 0.0,
        max_depth: 1,
    }
}

type Row = (String, String, String, String, String);

/// Everything an engine says, folded so a refusal from the store engine (`Err`)
/// and from the in-memory engine (`Unavailable`) compare as the same kind.
#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    available: bool,
    refusal: Option<String>,
    items: Vec<Row>,
    shown: u32,
    hidden: u32,
    total: u32,
    truncated: bool,
    walk_incomplete: Option<String>,
    note: Option<String>,
}

fn row(e: &ResolvedEdge) -> Row {
    (
        e.source_file.clone(),
        e.source_symbol.clone(),
        e.target_file.clone(),
        e.target_symbol.clone(),
        format!("{:?}", e.edge_kind),
    )
}

fn outcome(r: Result<Response<ResolvedEdge>, String>) -> Outcome {
    match r {
        Err(e) => Outcome {
            available: false,
            refusal: Some(e),
            items: vec![],
            shown: 0,
            hidden: 0,
            total: 0,
            truncated: false,
            walk_incomplete: None,
            note: None,
        },
        Ok(r) => {
            let (available, refusal) = match &r.resolution {
                ResolutionAvailability::Available => (true, None),
                ResolutionAvailability::Unavailable { reason } => (false, Some(reason.clone())),
            };
            Outcome {
                available,
                refusal,
                items: r.items.iter().map(row).collect(),
                shown: r.shown,
                hidden: r.hidden,
                total: r.total,
                truncated: r.truncated,
                walk_incomplete: r.walk_incomplete.clone(),
                note: r.note.clone(),
            }
        }
    }
}

fn store_answer(fx: &Fixture, query: &str, budget: u32) -> Outcome {
    outcome(
        StoreQueryEngine::new(&fx.store)
            .dependencies(request(query, budget))
            .map_err(|e| e.to_string()),
    )
}

fn memory_answer(fx: &Fixture, query: &str, budget: u32) -> Outcome {
    outcome(Ok(
        QueryEngine::new(&fx.extractions, &fx.resolution).dependencies(request(query, budget))
    ))
}

/// The inbound count inside a `walk_incomplete` note, or `None`.
fn inbound_count(note: &Option<String>) -> Option<u64> {
    let note = note.as_deref()?;
    let head = &note[..note.find(" inbound edge(s)")?];
    head.rsplit(' ').next()?.parse().ok()
}

/// A note with its inbound count blanked, so the generic parity probes do not
/// all fail on the one count disagreement that `p1_inbound_counts_agree` owns.
fn blank_count(note: &Option<String>) -> Option<String> {
    let count = inbound_count(note)?;
    note.as_ref()
        .map(|n| n.replace(&format!("{count} inbound"), "N inbound"))
}

/// Describe every field on which the engines differ ("" when they agree).
/// Order, the content of a *truncated* page and the inbound count are owned by
/// their own probes (`strict` false), so one root cause does not mask the rest.
fn diff(store: &Outcome, memory: &Outcome) -> String {
    diff_with(store, memory, false)
}

fn diff_with(store: &Outcome, memory: &Outcome, strict: bool) -> String {
    let (mut store, mut memory) = (store.clone(), memory.clone());
    if !strict {
        store.note = blank_count(&store.note).or(store.note);
        memory.note = blank_count(&memory.note).or(memory.note);
    }
    let (store, memory) = (&store, &memory);
    let mut out = String::new();
    let mut field = |name: &str, same: bool, a: String, b: String| {
        if !same {
            out.push_str(&format!("    {name}: store={a} memory={b}\n"));
        }
    };
    field(
        "available",
        store.available == memory.available,
        store.available.to_string(),
        memory.available.to_string(),
    );
    field(
        "refusal",
        store.refusal == memory.refusal,
        format!("{:?}", store.refusal),
        format!("{:?}", memory.refusal),
    );
    let a: BTreeSet<_> = store.items.iter().collect();
    let b: BTreeSet<_> = memory.items.iter().collect();
    field(
        "item set",
        (!strict && store.truncated) || (a == b && store.items.len() == memory.items.len()),
        format!(
            "{} items, only-store {:?}",
            store.items.len(),
            a.difference(&b).collect::<Vec<_>>()
        ),
        format!(
            "{} items, only-memory {:?}",
            memory.items.len(),
            b.difference(&a).collect::<Vec<_>>()
        ),
    );
    field(
        "item order",
        !strict || a != b || store.items == memory.items,
        format!("{:?}", store.items.iter().map(|r| &r.3).collect::<Vec<_>>()),
        format!(
            "{:?}",
            memory.items.iter().map(|r| &r.3).collect::<Vec<_>>()
        ),
    );
    field(
        "shown",
        store.shown == memory.shown,
        store.shown.to_string(),
        memory.shown.to_string(),
    );
    field(
        "hidden",
        store.hidden == memory.hidden,
        store.hidden.to_string(),
        memory.hidden.to_string(),
    );
    field(
        "total",
        store.total == memory.total,
        store.total.to_string(),
        memory.total.to_string(),
    );
    field(
        "truncated",
        store.truncated == memory.truncated,
        store.truncated.to_string(),
        memory.truncated.to_string(),
    );
    field(
        "walk_incomplete",
        store.walk_incomplete == memory.walk_incomplete,
        format!("{:?}", store.walk_incomplete),
        format!("{:?}", memory.walk_incomplete),
    );
    field(
        "note",
        store.note == memory.note,
        format!("{:?}", store.note),
        format!("{:?}", memory.note),
    );
    out
}

fn assert_parity(fx: &Fixture, queries: &[&str], budgets: &[u32]) {
    let mut report = String::new();
    for query in queries {
        for budget in budgets {
            let d = diff(
                &store_answer(fx, query, *budget),
                &memory_answer(fx, query, *budget),
            );
            if !d.is_empty() {
                report.push_str(&format!("  query {query:?} budget {budget}:\n{d}"));
            }
        }
    }
    assert!(report.is_empty(), "engines disagree:\n{report}");
}

const LIB: &str = "\
RETRY_BUDGET_FACTOR = 2


def helper():
    return 1


def other_helper():
    return 2


def leaf():
    return RETRY_BUDGET_FACTOR


def worker():
    helper()
    return other_helper()


class Svc:
    def run(self):
        return helper()
";

const CALLERS: &str = "\
from lib import worker, leaf


def caller_one():
    return worker()


def caller_two():
    return leaf()
";

const AMBIG_A: &str = "def target():\n    return 1\n\n\ndef dup():\n    return target()\n";
const AMBIG_B: &str = "def target():\n    return 2\n\n\ndef dup():\n    return target()\n";
const UNI: &str = "def aide():\n    return 1\n\n\ndef caf\u{e9}():\n    return aide()\n";

fn standard() -> Fixture {
    fixture(&[
        ("lib.py", LIB),
        ("callers.py", CALLERS),
        ("a.py", AMBIG_A),
        ("b.py", AMBIG_B),
        ("caf.py", UNI),
        (
            "pkg/mod.py",
            "def inner():\n    return 1\n\n\ndef outer():\n    return inner()\n",
        ),
    ])
}

// ---------------------------------------------------------------- probe 1

/// Probe 1. Engine parity on every query kind, at three budgets. Any
/// difference in items, total, truncated or walk_incomplete is a break.
#[test]
fn p1_engines_agree_on_every_query_kind() {
    let fx = standard();
    assert_parity(
        &fx,
        &[
            "lib.py",
            "callers.py",
            "worker",
            "Svc.run",
            "lib.py::worker",
            "lib.py::Svc.run",
            "RETRY_BUDGET_FACTOR",
            "dup",
            "a.py::dup",
            "no_such_symbol",
            "caf\u{e9}",
            "caf.py::caf\u{e9}",
            "",
            "   ",
            " worker ",
            "::worker",
            "lib.py::",
            "lib.py::nope",
            "nope.py::worker",
            "helper",
            "leaf",
        ],
        &[8_000, 25, 1],
    );
}

/// Probe 1, spelling. `./`, trailing slash, backslash, case and a directory.
/// Whatever each spelling means, both engines must mean the same thing.
#[test]
fn p1_engines_agree_on_path_spellings() {
    let fx = standard();
    assert_parity(
        &fx,
        &[
            "./lib.py",
            "lib.py/",
            "/lib.py",
            "pkg\\mod.py",
            "pkg/mod.py",
            "./pkg/mod.py",
            "pkg//mod.py",
            "pkg/../lib.py",
            "LIB.PY",
            "Lib.py",
            "pkg",
            "pkg/",
            "./pkg",
            ".",
            "./",
        ],
        &[8_000, 1],
    );
}

/// Probe 1, spelling. A spelling that does not name an indexed file must never
/// answer `Available` and empty with no qualification: that is "this file
/// depends on nothing", said about a file that was never looked up.
#[test]
fn p1_a_non_canonical_spelling_is_never_a_silent_empty_answer() {
    let fx = standard();
    let mut report = String::new();
    for query in [
        "./lib.py",
        "lib.py/",
        "/lib.py",
        "pkg\\mod.py",
        "./pkg/mod.py",
        "LIB.PY",
        "pkg",
        "pkg/",
        ".",
    ] {
        for (engine, o) in [
            ("store", store_answer(&fx, query, 8_000)),
            ("memory", memory_answer(&fx, query, 8_000)),
        ] {
            if o.available && o.items.is_empty() && o.walk_incomplete.is_none() {
                report.push_str(&format!(
                    "  {engine} {query:?}: Available, empty, no note\n"
                ));
            }
        }
    }
    assert!(report.is_empty(), "silent empty answers:\n{report}");
}

/// Probe 1, directory. A directory is not a file and not a symbol; the answer
/// must refuse (or at least not be empty-and-available).
#[test]
fn p1_a_directory_is_not_an_empty_dependency_list() {
    let fx = standard();
    for query in ["pkg", "pkg/"] {
        for (engine, o) in [
            ("store", store_answer(&fx, query, 8_000)),
            ("memory", memory_answer(&fx, query, 8_000)),
        ] {
            assert!(
                !(o.available && o.items.is_empty()),
                "{engine} {query:?}: a directory answered Available and empty: {o:?}"
            );
        }
    }
}

// ---------------------------------------------------------------- probe 2

/// Probe 2. `Svc.py` is both an indexed file and the spelling of method `py`
/// of class `Svc`. "File wins" is the documented choice; it must be announced,
/// because the symbol reading is silently unreachable by that spelling.
#[test]
fn p2_a_file_that_shadows_a_symbol_says_so() {
    let fx = fixture(&[
        ("Svc.py", "def own():\n    return 1\n\n\ndef uses():\n    return own()\n"),
        (
            "other.py",
            "class Svc:\n    def py(self):\n        return helper2()\n\n\ndef helper2():\n    return 1\n",
        ),
    ]);
    let mut report = String::new();
    for (engine, o) in [
        ("store", store_answer(&fx, "Svc.py", 8_000)),
        ("memory", memory_answer(&fx, "Svc.py", 8_000)),
    ] {
        assert!(o.available, "{engine}: {o:?}");
        assert!(
            o.items.iter().all(|r| r.0 == "Svc.py"),
            "{engine}: file must win: {o:?}"
        );
        let announced = o
            .note
            .as_deref()
            .is_some_and(|n| n.contains("symbol") || n.contains("shadow"));
        if !announced {
            report.push_str(&format!(
                "  {engine}: file wins silently; note={:?}\n",
                o.note
            ));
        }
    }
    assert!(report.is_empty(), "shadowing not announced:\n{report}");
    assert_parity(&fx, &["Svc.py", "other.py::Svc.py"], &[8_000]);
}

/// Probe 2. An unindexed path that spells a real symbol's file-qualified
/// form: `ghost.py::helper2` when only `other.py` has it.
#[test]
fn p2_an_unindexed_path_is_not_indexed_in_both_engines() {
    let fx = fixture(&[("other.py", "def helper2():\n    return 1\n")]);
    for query in ["ghost.py", "ghost.py::helper2", "other.py::ghost"] {
        let s = store_answer(&fx, query, 8_000);
        let m = memory_answer(&fx, query, 8_000);
        assert!(
            !s.available && !m.available,
            "{query}: store={s:?} memory={m:?}"
        );
    }
    assert_parity(
        &fx,
        &["ghost.py", "ghost.py::helper2", "other.py::ghost"],
        &[8_000],
    );
}

// ---------------------------------------------------------------- probe 3

const KINDS_PY: &str = "\
from typing import Optional
import os


class Base:
    def ping(self):
        return 1


class Child(Base):
    attr: Optional[Base] = None

    def run(self, other: Base) -> Optional[Base]:
        os.getcwd()
        return other.ping()


def uses_everything(x: Child) -> Child:
    c = Child()
    c.run(c)
    return c
";

const KINDS_TS: &str = "\
export interface Shape { area(): number }
export class Sq implements Shape {
  area(): number { return 1 }
}
export class Big extends Sq {}
export type Alias = Shape;
export function build(s: Shape): Alias { return new Big() }
";

const KINDS_GO: &str = "\
package kinds

type Doer interface{ Do() }

type Impl struct{ inner Doer }

func (i Impl) Do() {}

func Make() Doer { return Impl{} }

func Use(d Doer) { d.Do() }
";

const KINDS_RS: &str = "\
pub trait Speak { fn speak(&self) -> u8; }
pub struct Dog;
impl Speak for Dog { fn speak(&self) -> u8 { helper() } }
fn helper() -> u8 { 1 }
pub fn run(d: &Dog) -> u8 { d.speak() }
pub mod inner { pub fn deep() -> u8 { 1 } }
pub fn outer() -> u8 { inner::deep() }
pub fn recurse(n: u8) -> u8 { if n == 0 { 0 } else { recurse(n - 1) } }
";

const KINDS_JAVA: &str = "\
interface Greeter { String greet(); }
class Hello implements Greeter {
  public String greet() { return \"hi\"; }
}
class Main { String go(Hello h) { return h.greet(); } }
";

fn kinds_fixture() -> Fixture {
    fixture(&[
        ("kinds.py", KINDS_PY),
        ("kinds.ts", KINDS_TS),
        ("kinds.go", KINDS_GO),
        ("kinds.rs", KINDS_RS),
        ("Kinds.java", KINDS_JAVA),
        (
            "use_kinds.py",
            "from kinds import Child, uses_everything\nimport kinds\n\n\nclass Sub(Child):\n    pass\n\n\ndef go():\n    c = Child()\n    kinds.uses_everything(c)\n    return uses_everything(c)\n",
        ),
        (
            "app.ts",
            "import { Sq, build } from './kinds';\nexport class Mine extends Sq {}\nexport function make() { return build(new Mine()) }\n",
        ),
    ])
}

fn raw_rows(fx: &Fixture) -> Vec<devmap_store::StoredEdge> {
    fx.store.latest_edges(0.0).unwrap()
}

/// Probe 3. Symbol-level: for every source symbol that has a stored edge,
/// `deps <symbol>` must equal the stored rows whose `source_symbol` is that
/// symbol in that file — no kind dropped, nothing extra — in both engines.
#[test]
fn p3_symbol_dependencies_equal_the_stored_outbound_rows() {
    let fx = kinds_fixture();
    let raw = raw_rows(&fx);
    assert!(
        raw.len() > 20,
        "fixture must produce a real graph: {}",
        raw.len()
    );
    let kinds: BTreeSet<&str> = raw.iter().map(|e| e.edge_kind.as_str()).collect();
    eprintln!("edge kinds in fixture: {kinds:?}");
    let sources: BTreeSet<(String, String)> = raw
        .iter()
        .map(|e| (e.source_file.clone(), e.source_symbol.clone()))
        .collect();
    let mut report = String::new();
    for (file, symbol) in &sources {
        let expected_rows: Vec<(String, String, String)> = {
            let mut v: Vec<_> = raw
                .iter()
                .filter(|e| &e.source_file == file && &e.source_symbol == symbol)
                .map(|e| {
                    (
                        e.target_file.clone(),
                        e.target_symbol.clone(),
                        e.edge_kind.clone(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        let expected: BTreeSet<(String, String, String)> = expected_rows.iter().cloned().collect();
        let expected = &expected;
        let expected_len = expected_rows.len();
        let query = if symbol.contains("::") {
            symbol.clone()
        } else {
            format!("{file}::{symbol}")
        };
        for (engine, o) in [
            ("store", store_answer(&fx, &query, 200_000)),
            ("memory", memory_answer(&fx, &query, 200_000)),
        ] {
            if !o.available {
                report.push_str(&format!(
                    "  {engine} {query}: unavailable {:?}\n",
                    o.refusal
                ));
                continue;
            }
            let got: BTreeSet<(String, String, String)> = o
                .items
                .iter()
                .map(|r| (r.2.clone(), r.3.clone(), r.4.clone()))
                .collect();
            if &got != expected || o.items.len() != expected_len {
                report.push_str(&format!(
                    "  {engine} {query}: missing {:?} extra {:?} (got {} want {})\n",
                    expected.difference(&got).collect::<Vec<_>>(),
                    got.difference(expected).collect::<Vec<_>>(),
                    o.items.len(),
                    expected_len
                ));
            }
            if o.total as usize != expected_len {
                report.push_str(&format!(
                    "  {engine} {query}: total {} want {}\n",
                    o.total, expected_len
                ));
            }
        }
    }
    assert!(report.is_empty(), "outbound completeness:\n{report}");
}

/// Probe 3, file level. Every stored edge whose `source_file` is the file —
/// including those whose source is a symbol inside it — is outbound from the
/// file, in both engines.
#[test]
fn p3_file_dependencies_include_edges_sourced_by_inner_symbols() {
    let fx = kinds_fixture();
    let raw = raw_rows(&fx);
    let files: BTreeSet<&str> = raw.iter().map(|e| e.source_file.as_str()).collect();
    let mut report = String::new();
    for file in files {
        let expected = raw.iter().filter(|e| e.source_file == file).count();
        let inbound = raw
            .iter()
            .filter(|e| e.target_file == file && e.source_file != file)
            .count();
        for (engine, o) in [
            ("store", store_answer(&fx, file, 200_000)),
            ("memory", memory_answer(&fx, file, 200_000)),
        ] {
            if !o.available || o.items.len() != expected || o.total as usize != expected {
                report.push_str(&format!(
                    "  {engine} {file}: available={} items={} total={} want {expected} (inbound {inbound})\n",
                    o.available,
                    o.items.len(),
                    o.total
                ));
            }
            if o.items.iter().any(|r| r.0 != file) {
                report.push_str(&format!("  {engine} {file}: foreign-source row present\n"));
            }
        }
    }
    assert!(report.is_empty(), "file outbound completeness:\n{report}");
}

/// Probe 3 + 1 on the kinds fixture: parity for every file, every source
/// symbol, at a squeezing budget.
#[test]
fn p3_engines_agree_on_every_file_and_symbol_of_the_kinds_fixture() {
    let fx = kinds_fixture();
    let raw = raw_rows(&fx);
    let mut queries: BTreeSet<String> = BTreeSet::new();
    for e in &raw {
        queries.insert(e.source_file.clone());
        queries.insert(e.target_file.clone());
        queries.insert(if e.source_symbol.contains("::") {
            e.source_symbol.clone()
        } else {
            format!("{}::{}", e.source_file, e.source_symbol)
        });
        queries.insert(if e.target_symbol.contains("::") {
            e.target_symbol.clone()
        } else {
            format!("{}::{}", e.target_file, e.target_symbol)
        });
    }
    let list: Vec<&str> = queries.iter().map(String::as_str).collect();
    assert_parity(&fx, &list, &[200_000, 60, 1]);
}

// ---------------------------------------------------------------- probe 4

fn many_inbound(n: usize) -> Fixture {
    let mut hub = String::from("def hub():\n    return 1\n");
    let mut users = String::from("from hub import hub\n\n");
    for i in 0..n {
        users.push_str(&format!("def user_{i}():\n    return hub()\n\n"));
    }
    hub.push('\n');
    fixture(&[("hub.py", hub.leak()), ("users.py", users.leak())])
}

/// Probe 4. A target with zero outbound and many inbound, at budget 1:
/// nothing outbound is hidden, so `truncated` must be false, `total` 0, and
/// `hidden` 0.
#[test]
fn p4_zero_outbound_many_inbound_is_not_truncated() {
    let fx = many_inbound(60);
    for query in ["hub", "hub.py::hub"] {
        for (engine, o) in [
            ("store", store_answer(&fx, query, 1)),
            ("memory", memory_answer(&fx, query, 1)),
        ] {
            assert!(o.available, "{engine} {query}: {o:?}");
            assert!(
                !o.truncated,
                "{engine} {query}: truncated with nothing hidden: {o:?}"
            );
            assert_eq!((o.total, o.hidden, o.shown), (0, 0, 0), "{engine} {query}");
            let note = o.note.clone().unwrap_or_default();
            let truth = raw_rows(&fx)
                .iter()
                .filter(|e| e.target_symbol == "hub.py::hub" && e.source_symbol != e.target_symbol)
                .count() as u64;
            assert!(truth >= 60, "fixture: {truth}");
            assert_eq!(
                inbound_count(&o.note),
                Some(truth),
                "{engine} {query}: the stated inbound count must be the stored one: {note:?}"
            );
        }
    }
    assert_parity(
        &fx,
        &["hub", "hub.py::hub", "hub.py", "users.py"],
        &[1, 25, 8_000],
    );
}

/// Probe 4. An all-inbound *file* at budget 1 (the lane's own test uses 8000).
#[test]
fn p4_an_all_inbound_file_at_budget_one_is_not_truncated() {
    let fx = many_inbound(30);
    // hub.py has a declaration edge, so strip it the way the lane's test does.
    let mut resolution = fx.resolution.clone();
    resolution.edges.retain(|e| e.source_file != "hub.py");
    let analysis = devmap_analyze::analyze(&fx.extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &fx.extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    let stripped = Fixture {
        extractions: fx.extractions.clone(),
        resolution,
        store,
    };
    for (engine, o) in [
        ("store", store_answer(&stripped, "hub.py", 1)),
        ("memory", memory_answer(&stripped, "hub.py", 1)),
    ] {
        assert!(o.available && o.items.is_empty(), "{engine}: {o:?}");
        assert!(
            !o.truncated && o.hidden == 0 && o.total == 0,
            "{engine}: {o:?}"
        );
        let truth = stripped
            .resolution
            .edges
            .iter()
            .filter(|e| e.target_file == "hub.py" && e.source_file != "hub.py")
            .count() as u64;
        assert_eq!(inbound_count(&o.note), Some(truth), "{engine}: {o:?}");
    }
    assert_parity(&stripped, &["hub.py", "hub", "hub.py::hub"], &[1, 8_000]);
}

/// Probe 4. `truncated` is true exactly when outbound items are hidden, and
/// `shown + hidden == total == outbound count` across a range of budgets.
#[test]
fn p4_truncated_means_outbound_items_are_hidden() {
    let fx = standard();
    let outbound = fx
        .resolution
        .edges
        .iter()
        .filter(|e| e.source_file == "lib.py")
        .count();
    for budget in [1, 24, 25, 26, 49, 50, 51, 99, 100, 8_000] {
        for (engine, o) in [
            ("store", store_answer(&fx, "lib.py", budget)),
            ("memory", memory_answer(&fx, "lib.py", budget)),
        ] {
            assert_eq!(o.total as usize, outbound, "{engine} budget {budget}");
            assert_eq!(o.shown + o.hidden, o.total, "{engine} budget {budget}");
            assert_eq!(o.truncated, o.hidden > 0, "{engine} budget {budget}: {o:?}");
            assert_eq!(o.items.len() as u32, o.shown, "{engine} budget {budget}");
        }
    }
    assert_parity(
        &fx,
        &["lib.py", "worker", "lib.py::worker"],
        &[1, 24, 25, 26, 49, 50, 51, 99, 100, 8_000],
    );
}

/// Probe 4. 5 000 outbound edges stay bounded in time and in output size, at
/// file and symbol level, in both engines.
#[test]
fn p4_a_five_thousand_edge_answer_stays_bounded() {
    let n = 5_000usize;
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!("def callee_{i}():\n    return {i}\n\n\n"));
    }
    src.push_str("def fan():\n");
    for i in 0..n {
        src.push_str(&format!("    callee_{i}()\n"));
    }
    let fx = fixture(&[("big.py", src.leak())]);
    let outbound = fx
        .resolution
        .edges
        .iter()
        .filter(|e| e.source_file == "big.py")
        .count();
    assert!(
        outbound >= n,
        "fixture must hold the outbound set: {outbound}"
    );
    for query in ["big.py", "fan", "big.py::fan"] {
        for (engine, run) in [
            ("store", store_answer as fn(&Fixture, &str, u32) -> Outcome),
            (
                "memory",
                memory_answer as fn(&Fixture, &str, u32) -> Outcome,
            ),
        ] {
            let started = Instant::now();
            let o = run(&fx, query, 2_000);
            let took = started.elapsed();
            assert!(took < Duration::from_secs(5), "{engine} {query}: {took:?}");
            assert!(o.available, "{engine} {query}: {o:?}");
            assert!(
                o.truncated && o.hidden > 0,
                "{engine} {query}: must page, {o:?}"
            );
            assert!(
                o.items.len() <= 200,
                "{engine} {query}: {} items for a 2000 budget",
                o.items.len()
            );
            assert!(o.total as usize >= n, "{engine} {query}: total {}", o.total);
            assert_eq!(o.shown + o.hidden, o.total, "{engine} {query}");
        }
    }
    assert_parity(&fx, &["big.py", "fan", "big.py::fan"], &[2_000, 1]);
}

// ---------------------------------------------------------------- probe 5

/// Probe 5. `walk_incomplete` is documented as "None on a complete answer"
/// (`Response::walk_incomplete`). An honest, complete, empty answer for a leaf
/// symbol carries an informational note there, so any client that treats the
/// field as the partial-answer flag (the session report counts it as
/// `walk_incomplete`; the CLI prints it as `warning:`) reads a complete answer
/// as partial. This probe asserts the contract; its failure is the finding.
#[test]
fn p5_a_complete_answer_carries_no_walk_incomplete() {
    let fx = standard();
    for (engine, o) in [
        ("store", store_answer(&fx, "RETRY_BUDGET_FACTOR", 8_000)),
        ("memory", memory_answer(&fx, "RETRY_BUDGET_FACTOR", 8_000)),
    ] {
        assert!(
            o.available && o.items.is_empty() && !o.truncated,
            "{engine}: {o:?}"
        );
        assert!(
            o.walk_incomplete.is_none(),
            "{engine}: complete answer flagged walk_incomplete={:?}",
            o.walk_incomplete
        );
    }
}

/// Probe 5. The pointer is machine-stable: its count equals the inbound edges
/// actually in the store (distinct from the edge-row count of callers: same
/// caller twice counts twice), and the note names `impact` verbatim.
#[test]
fn p5_the_inbound_count_equals_the_stored_inbound_rows() {
    let fx = standard();
    let raw = raw_rows(&fx);
    let inbound = raw
        .iter()
        .filter(|e| {
            e.target_symbol.ends_with("RETRY_BUDGET_FACTOR") && e.source_symbol != e.target_symbol
        })
        .count();
    assert!(inbound > 0);
    for (engine, o) in [
        ("store", store_answer(&fx, "RETRY_BUDGET_FACTOR", 8_000)),
        ("memory", memory_answer(&fx, "RETRY_BUDGET_FACTOR", 8_000)),
    ] {
        let note = o.note.unwrap_or_default();
        assert!(
            note.contains(&format!("{inbound} inbound edge(s)")),
            "{engine}: want {inbound}: {note:?}"
        );
        assert!(note.contains("`impact`"), "{engine}: {note:?}");
    }
}

/// Probe 5. When an outbound page is non-empty the note is absent, and when a
/// budget hides items the reason is the budget's, not the inbound note.
#[test]
fn p5_the_note_does_not_ride_on_a_non_empty_answer() {
    let fx = standard();
    for query in ["worker", "lib.py"] {
        for (engine, o) in [
            ("store", store_answer(&fx, query, 8_000)),
            ("memory", memory_answer(&fx, query, 8_000)),
        ] {
            assert!(!o.items.is_empty(), "{engine} {query}");
            assert!(
                !o.note.unwrap_or_default().contains("inbound edge(s)"),
                "{engine} {query}"
            );
        }
    }
}

/// Probe 5. A call-blind target (HCL, no call extractor): the store engine
/// refuses an empty answer because nothing was measured; the in-memory engine
/// must not report the same symbol as an available empty leaf.
#[test]
fn p5_a_call_blind_leaf_is_not_an_available_empty_answer_in_either_engine() {
    let fx = fixture(&[
        ("lib.py", "def helper():\n    return 1\n"),
        (
            "main.tf",
            "variable \"region\" {\n  default = \"us-east-1\"\n}\n\noutput \"region_out\" {\n  value = var.region\n}\n",
        ),
    ]);
    let store = store_answer(&fx, "main.tf::variable.region", 8_000);
    let memory = memory_answer(&fx, "main.tf::variable.region", 8_000);
    assert!(
        !(memory.available && memory.items.is_empty())
            || (store.available && store.items.is_empty()),
        "engines disagree on a call-blind leaf: store={store:?} memory={memory:?}"
    );
    assert_parity(
        &fx,
        &["main.tf::variable.region", "main.tf", "variable.region"],
        &[8_000],
    );
}

// ---------------------------------------------------------------- probe 6

/// Probe 6. Two namesakes, one with outbound edges and one without (a leaf
/// constant and a function share a name): the bare name is refused with both
/// candidates in both engines, and each `file::symbol` answers its own side.
#[test]
fn p6_namesakes_where_only_one_has_outbound_edges() {
    let fx = fixture(&[
        (
            "a.py",
            "def thing():\n    return b()\n\n\ndef b():\n    return 1\n",
        ),
        ("b.py", "thing = 3\n"),
        (
            "c.py",
            "from b import thing\n\n\ndef use():\n    return thing\n",
        ),
    ]);
    let s = store_answer(&fx, "thing", 8_000);
    let m = memory_answer(&fx, "thing", 8_000);
    for (engine, o) in [("store", &s), ("memory", &m)] {
        assert!(!o.available, "{engine}: must refuse: {o:?}");
        let reason = o.refusal.clone().unwrap_or_default();
        assert!(
            reason.contains("a.py::thing") && reason.contains("b.py::thing"),
            "{engine}: {reason}"
        );
    }
    for (engine, o) in [
        ("store", store_answer(&fx, "a.py::thing", 8_000)),
        ("memory", memory_answer(&fx, "a.py::thing", 8_000)),
    ] {
        assert!(o.available && !o.items.is_empty(), "{engine}: {o:?}");
        assert!(o.items.iter().all(|r| r.0 == "a.py"), "{engine}");
    }
    for (engine, o) in [
        ("store", store_answer(&fx, "b.py::thing", 8_000)),
        ("memory", memory_answer(&fx, "b.py::thing", 8_000)),
    ] {
        assert!(o.available && o.items.is_empty(), "{engine}: {o:?}");
    }
    assert_parity(&fx, &["thing", "a.py::thing", "b.py::thing"], &[8_000, 1]);
}

/// Probe 6. The same name across languages.
#[test]
fn p6_a_name_shared_across_languages_is_refused_with_both_candidates() {
    let fx = fixture(&[
        (
            "x.py",
            "def render():\n    return 1\n\n\ndef go():\n    return render()\n",
        ),
        (
            "y.ts",
            "export function render() { return 2 }\nexport function go() { return render() }\n",
        ),
        (
            "z.go",
            "package z\n\nfunc render() int { return 3 }\n\nfunc Go() int { return render() }\n",
        ),
    ]);
    for (engine, o) in [
        ("store", store_answer(&fx, "render", 8_000)),
        ("memory", memory_answer(&fx, "render", 8_000)),
    ] {
        assert!(!o.available, "{engine}: {o:?}");
        let reason = o.refusal.clone().unwrap_or_default();
        for f in ["x.py::render", "y.ts::render", "z.go::render"] {
            assert!(reason.contains(f), "{engine} missing {f}: {reason}");
        }
    }
    assert_parity(
        &fx,
        &[
            "render",
            "go",
            "x.py::render",
            "y.ts::render",
            "z.go::render",
        ],
        &[8_000],
    );
}

/// Probe 6. A name that is ambiguous only on the inbound side (a symbol that
/// is a callee in two files and a caller in none) must still be refused: the
/// lane unions both sides to decide ambiguity.
#[test]
fn p6_a_callee_only_namesake_pair_is_refused() {
    let fx = fixture(&[
        ("a.py", "def shared():\n    return 1\n"),
        ("b.py", "def shared():\n    return 2\n"),
        (
            "c.py",
            "from a import shared\n\n\ndef use():\n    return shared()\n",
        ),
    ]);
    assert_parity(&fx, &["shared", "a.py::shared", "b.py::shared"], &[8_000]);
}

// ---------------------------------------------------------------- confidence floor

/// The floor applies to outbound rows, to the inbound count and to the
/// ordering identically in both engines.
#[test]
fn p1_engines_agree_under_a_confidence_floor() {
    let fx = standard();
    let mut report = String::new();
    for floor in [0.0f32, 0.5, 0.9, 1.0] {
        for query in [
            "lib.py",
            "worker",
            "RETRY_BUDGET_FACTOR",
            "callers.py::caller_one",
        ] {
            let req = |q: &str| Request {
                query: q.to_string(),
                token_budget: 8_000,
                min_confidence: floor,
                max_depth: 1,
            };
            let s = outcome(
                StoreQueryEngine::new(&fx.store)
                    .dependencies(req(query))
                    .map_err(|e| e.to_string()),
            );
            let m = outcome(Ok(
                QueryEngine::new(&fx.extractions, &fx.resolution).dependencies(req(query))
            ));
            let d = diff(&s, &m);
            if !d.is_empty() {
                report.push_str(&format!("  floor {floor} query {query:?}:\n{d}"));
            }
        }
    }
    assert!(report.is_empty(), "floor parity:\n{report}");
}

/// Probe 5. `min_rung` can empty a page that had edges before the cut. The
/// inbound note is then built from the *post-cut* emptiness and says
/// "depends on nothing in the index" about a symbol the rung histogram shows
/// has dependencies (they were filtered). A false positive claim.
#[test]
fn p5_a_rung_that_filters_every_edge_does_not_claim_the_target_depends_on_nothing() {
    use devmap_query::rung::Rung;
    let extractions = vec![
        extract_file("lib_a.py", "def shared():\n    return 1\n"),
        extract_file("lib_b.py", "def shared():\n    return 2\n"),
        extract_file(
            "app.py",
            "def helper():\n    return shared()\n\n\ndef main():\n    return helper()\n",
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let mut resolution = resolver.resolve_all(&extractions).unwrap();
    crate::common::dispatch_ambiguous_sites(&mut resolution);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    let engine = StoreQueryEngine::new(&store);
    let open = engine
        .dependencies(request("app.py::helper", 8_000))
        .unwrap();
    assert!(
        !open.items.is_empty(),
        "fixture: helper must have dispatch edges: {open:?}"
    );
    let floored = engine
        .dependencies_at_rung(request("app.py::helper", 8_000), Some(Rung::Deterministic))
        .unwrap();
    assert!(
        floored.items.is_empty(),
        "fixture: the floor must remove them: {floored:?}"
    );
    let filtered = floored.rungs.as_ref().map_or(0, |h| h.filtered_out);
    assert!(filtered > 0, "histogram must show what the rung hid");
    let note = floored.note.clone().unwrap_or_default();
    assert!(
        !note.contains("depends on nothing"),
        "helper has {filtered} edge(s) hidden by the rung, yet the answer says: {note}"
    );
}

// ------------------------------------------------- strict parity (own causes)

/// Probe 1 / 5. The inbound count the note states must agree between engines
/// and equal the number of distinct stored inbound edges, at every floor.
#[test]
fn p1_inbound_counts_agree_between_engines_and_with_the_store() {
    let fx = standard();
    let raw = raw_rows(&fx);
    let mut report = String::new();
    for query in [
        "RETRY_BUDGET_FACTOR",
        "lib.py::RETRY_BUDGET_FACTOR",
        "b.py::dup",
        "lib.py::Svc.run",
    ] {
        let s = store_answer(&fx, query, 8_000);
        let m = memory_answer(&fx, query, 8_000);
        let (sc, mc) = (inbound_count(&s.note), inbound_count(&m.note));
        let want = raw
            .iter()
            .filter(|e| {
                (e.target_symbol == query
                    || format!(
                        "{}::{}",
                        e.target_file,
                        e.target_symbol.rsplit("::").next().unwrap_or("")
                    ) == query
                    || e.target_symbol.ends_with(&format!("::{query}")))
                    && e.source_symbol != e.target_symbol
            })
            .count() as u64;
        if sc != mc || (sc.is_some() && sc != Some(want)) {
            report.push_str(&format!(
                "  {query}: store={sc:?} memory={mc:?} stored-rows={want}\n"
            ));
        }
    }
    assert!(report.is_empty(), "inbound counts:\n{report}");
}

/// Probe 1. Item order agrees between engines when nothing is truncated: a
/// client diffing the two, or a golden file, sees one order.
#[test]
fn p1_item_order_agrees_between_engines() {
    let fx = standard();
    let mut report = String::new();
    for query in ["lib.py", "callers.py", "worker", "pkg/mod.py"] {
        let d = diff_with(
            &store_answer(&fx, query, 200_000),
            &memory_answer(&fx, query, 200_000),
            true,
        );
        if d.contains("item order") {
            report.push_str(&format!("  {query}:\n{d}"));
        }
    }
    assert!(report.is_empty(), "order:\n{report}");
}

/// Probe 1 / 4. A *truncated* page holds the same edges in both engines: the
/// same query at the same budget must not show different rows depending on
/// which engine answered.
#[test]
fn p1_a_truncated_page_has_the_same_rows_in_both_engines() {
    let fx = standard();
    let mut report = String::new();
    for query in ["lib.py", "callers.py"] {
        for budget in [25, 50, 100] {
            let d = diff_with(
                &store_answer(&fx, query, budget),
                &memory_answer(&fx, query, budget),
                true,
            );
            if d.contains("item set") {
                report.push_str(&format!("  {query} b={budget}:\n{d}"));
            }
        }
    }
    assert!(report.is_empty(), "truncated pages:\n{report}");
}

/// Probe 4. A file's squeezed page (budget for one row) should lead with a
/// dependency, not with the file's own `Contains` declaration of one of its
/// symbols, which depends on nothing outside the file. The lane's fix exists so
/// that "a budgeted page filled with callers first"; here it fills with
/// self-declarations first instead.
#[test]
fn p4_a_squeezed_file_page_does_not_lead_with_its_own_declarations() {
    let fx = standard();
    for (engine, o) in [
        ("store", store_answer(&fx, "callers.py", 25)),
        ("memory", memory_answer(&fx, "callers.py", 25)),
    ] {
        assert_eq!(o.items.len(), 1, "{engine}: {o:?}");
        assert!(
            o.items[0].4 != "Contains" && o.items[0].4 != "MemberOf",
            "{engine}: the one visible row is a declaration edge: {:?}",
            o.items[0]
        );
    }
}

/// Probe 4. Count agreement on a large inbound population, and the work stays
/// bounded: the store computes the inbound count by walking the inbound edges
/// once per *start*; with duplicate starts that is quadratic.
#[test]
fn p4_a_large_inbound_population_is_counted_once_and_cheaply() {
    let fx = many_inbound(3_000);
    let truth = raw_rows(&fx)
        .iter()
        .filter(|e| e.target_symbol == "hub.py::hub" && e.source_symbol != e.target_symbol)
        .count() as u64;
    let started = Instant::now();
    let s = store_answer(&fx, "hub", 8_000);
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "store took {took:?} for {truth} inbound edges"
    );
    assert_eq!(
        inbound_count(&s.note),
        Some(truth),
        "{:?}",
        s.walk_incomplete
    );
    let m = memory_answer(&fx, "hub", 8_000);
    assert_eq!(
        inbound_count(&m.note),
        Some(truth),
        "{:?}",
        m.walk_incomplete
    );
}

/// Probe 3. A self edge (recursion) is outbound from the symbol exactly once
/// and is never counted as inbound; both engines agree. A nested module path
/// (`inner::deep`) round-trips through `file::symbol`.
#[test]
fn p3_recursion_and_nested_module_symbols_resolve() {
    let fx = kinds_fixture();
    let mut report = String::new();
    for query in [
        "kinds.rs::recurse",
        "recurse",
        "kinds.rs::outer",
        "outer",
        "deep",
        "kinds.rs::inner::deep",
        "kinds.rs::inner.deep",
    ] {
        let s = store_answer(&fx, query, 8_000);
        let m = memory_answer(&fx, query, 8_000);
        report.push_str(&format!(
            "  {query}: store avail={} items={:?} / memory avail={} items={}\n",
            s.available,
            s.items
                .iter()
                .map(|r| (&r.1, &r.3, &r.4))
                .collect::<Vec<_>>(),
            m.available,
            m.items.len()
        ));
        if !s.available || s.items.is_empty() {
            report.push_str(&format!(
                "    ^ no outbound edges for {query}: {:?}\n",
                s.refusal
            ));
        }
    }
    eprintln!("{report}");
    let recurse = store_answer(&fx, "kinds.rs::recurse", 8_000);
    assert!(
        recurse
            .items
            .iter()
            .any(|r| r.1.ends_with("recurse") && r.3.ends_with("recurse")),
        "the self-call must be an outbound edge of recurse: {recurse:?}"
    );
    let outer = store_answer(&fx, "outer", 8_000);
    assert!(
        outer.available && outer.items.iter().any(|r| r.3.ends_with("deep")),
        "outer calls inner::deep: {outer:?}"
    );
    assert_parity(
        &fx,
        &["kinds.rs::recurse", "recurse", "outer", "deep"],
        &[8_000],
    );
}
