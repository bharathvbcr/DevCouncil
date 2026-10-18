//! `dependencies` answers "what does the target depend on": outbound edges.
//!
//! Before this, the tool read a file's edges with *either* endpoint equal to the
//! file, ordered by confidence, so the visible budget page filled with callers
//! first; and a symbol name was looked up as a file path and answered
//! "<name> is not indexed" for a symbol `impact` and `search` resolve. Every
//! test here runs against both engines, which share the contract.

use devmap_extract::model::Extraction;
use devmap_extract::extract_file;
use devmap_query::{
    QueryEngine, Request, ResolutionAvailability, Response, StoreQueryEngine,
};
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

struct Fixture {
    extractions: Vec<Extraction>,
    resolution: ResolutionResult,
    store: Store,
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    fixture_with(files, |_| {})
}

/// `edit` reshapes the resolution before both engines see it, for a population
/// the extractor will not produce on its own (a file with callers and no edge
/// leaving it — the extractor always writes a declaration edge for a symbol).
fn fixture_with(
    files: &[(&str, &str)],
    edit: impl FnOnce(&mut ResolutionResult),
) -> Fixture {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let mut resolution = resolver.resolve_all(&extractions).unwrap();
    edit(&mut resolution);
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

/// The same question to both engines. A refusal (an ambiguous bare name) is an
/// `Err` from the store engine and an `Unavailable` answer from the in-memory
/// one, so both are folded to `Result<Response, String>`.
fn ask_both(fx: &Fixture, query: &str, budget: u32) -> [(&'static str, Result<Response<ResolvedEdge>, String>); 2] {
    let store = StoreQueryEngine::new(&fx.store)
        .dependencies(request(query, budget))
        .map_err(|e| e.to_string());
    let memory = Ok(QueryEngine::new(&fx.extractions, &fx.resolution)
        .dependencies(request(query, budget)));
    [("store", store), ("memory", memory)]
}

fn available(r: &Response<ResolvedEdge>) -> bool {
    matches!(r.resolution, ResolutionAvailability::Available)
}

fn reason(r: &Response<ResolvedEdge>) -> String {
    match &r.resolution {
        ResolutionAvailability::Unavailable { reason } => reason.clone(),
        _ => String::new(),
    }
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

fn lib_and_callers() -> Fixture {
    fixture(&[("lib.py", LIB), ("callers.py", CALLERS)])
}

/// Criterion 4, and the core of criterion 1: one inbound and one outbound edge.
#[test]
fn a_symbol_answers_its_outbound_edges_not_its_callers() {
    let fx = lib_and_callers();
    for (engine, answer) in ask_both(&fx, "worker", 8_000) {
        let answer = answer.unwrap();
        assert!(available(&answer), "{engine}: {}", reason(&answer));
        let targets: Vec<&str> = answer.items.iter().map(|e| e.target_symbol.as_str()).collect();
        assert!(
            answer.items.iter().all(|e| e.source_symbol.ends_with("worker")),
            "{engine}: every edge must leave worker, got {:?}",
            answer.items
        );
        assert!(
            targets.iter().any(|t| t.ends_with("helper")) && targets.iter().any(|t| t.ends_with("other_helper")),
            "{engine}: {targets:?}"
        );
        assert_eq!(answer.total as usize, answer.items.len(), "{engine}");
    }
}

/// Criterion 1: a symbol name is never "not indexed" when it is indexed, and a
/// constant that only others reference has an honest empty answer that names
/// the inbound edges it left out.
#[test]
fn a_symbol_with_no_outbound_edges_is_an_honest_empty_answer() {
    let fx = lib_and_callers();
    for (engine, answer) in ask_both(&fx, "RETRY_BUDGET_FACTOR", 8_000) {
        let answer = answer.unwrap();
        assert!(
            available(&answer),
            "{engine}: must not be unavailable: {}",
            reason(&answer)
        );
        assert!(!reason(&answer).contains("not indexed"), "{engine}");
        assert!(answer.items.is_empty(), "{engine}: {:?}", answer.items);
        assert_eq!(answer.total, 0, "{engine}");
        let note = answer.note.clone().unwrap_or_default();
        assert!(
            note.contains("inbound edge(s)") && note.contains("impact"),
            "{engine}: the inbound edges left out must be named: {note:?}"
        );
    }
}

/// A name nothing indexes is still "not indexed" — the honest empty answer is
/// for an indexed symbol, not for any string.
#[test]
fn an_unknown_name_is_not_indexed() {
    let fx = lib_and_callers();
    for (engine, answer) in ask_both(&fx, "no_such_symbol", 8_000) {
        let answer = answer.unwrap();
        assert!(!available(&answer), "{engine}");
        assert!(reason(&answer).contains("not indexed"), "{engine}");
    }
}

/// Criterion 2 and 3: a file's page is edges written in the file, and a budget
/// that fits one edge spends it on an outbound edge, not a caller.
#[test]
fn a_file_answers_edges_written_in_it_and_the_budget_goes_to_them() {
    let fx = lib_and_callers();
    for budget in [8_000, 25, 1] {
        for (engine, answer) in ask_both(&fx, "lib.py", budget) {
            let answer = answer.unwrap();
            assert!(available(&answer), "{engine}: {}", reason(&answer));
            assert!(
                answer.items.iter().all(|e| e.source_file == "lib.py"),
                "{engine} budget {budget}: {:?}",
                answer.items
            );
            assert!(answer.total > 0, "{engine}");
        }
    }
}

/// `truncated` and `total` count outbound edges only: a file with outbound
/// edges and callers reports the outbound population.
#[test]
fn total_and_truncated_count_outbound_only() {
    let fx = lib_and_callers();
    let [(_, store), (_, memory)] = ask_both(&fx, "lib.py", 8_000);
    let full_store = store.unwrap();
    let full_memory = memory.unwrap();
    assert!(!full_store.truncated && !full_memory.truncated);
    let outbound = fx
        .resolution
        .edges
        .iter()
        .filter(|e| e.source_file == "lib.py")
        .count();
    assert_eq!(full_store.total as usize, outbound);
    assert_eq!(full_memory.total as usize, outbound);
    for (engine, answer) in ask_both(&fx, "lib.py", 1) {
        let answer = answer.unwrap();
        assert_eq!(answer.total as usize, outbound, "{engine}");
        assert_eq!(answer.shown + answer.hidden, answer.total, "{engine}");
        assert!(answer.truncated, "{engine}");
    }
}

/// A file with callers and nothing leaving it is withheld, not paged: the answer
/// is empty and says how many inbound edges there were.
#[test]
fn an_all_inbound_file_is_withheld_and_labelled() {
    let fx = fixture_with(
        &[
            ("defs.py", "VALUE = 1\n"),
            ("user.py", "from defs import VALUE\n\n\ndef use():\n    return VALUE\n"),
        ],
        |resolution| resolution.edges.retain(|e| e.source_file != "defs.py"),
    );
    let inbound = fx
        .resolution
        .edges
        .iter()
        .filter(|e| e.target_file == "defs.py" && e.source_file != "defs.py")
        .count();
    let outbound = fx
        .resolution
        .edges
        .iter()
        .filter(|e| e.source_file == "defs.py")
        .count();
    assert!(inbound > 0, "fixture must have an inbound edge");
    assert_eq!(outbound, 0, "fixture must have no outbound edge");
    for (engine, answer) in ask_both(&fx, "defs.py", 8_000) {
        let answer = answer.unwrap();
        assert!(available(&answer), "{engine}");
        assert!(answer.items.is_empty(), "{engine}: {:?}", answer.items);
        let note = answer.note.clone().unwrap_or_default();
        assert!(
            note.contains(&format!("{inbound} inbound edge(s)")),
            "{engine}: the count must be stated: {note:?}"
        );
    }
}

/// `Type.method` resolves through the matcher `impact` uses.
#[test]
fn a_qualified_method_resolves() {
    let fx = lib_and_callers();
    for (engine, answer) in ask_both(&fx, "Svc.run", 8_000) {
        let answer = answer.unwrap();
        assert!(available(&answer), "{engine}: {}", reason(&answer));
        assert!(
            answer.items.iter().any(|e| e.target_symbol.ends_with("helper")),
            "{engine}: {:?}",
            answer.items
        );
        assert!(
            answer.items.iter().all(|e| e.source_symbol.ends_with("Svc.run")),
            "{engine}: {:?}",
            answer.items
        );
    }
}

/// A non-ASCII name is matched exactly like any other.
#[test]
fn a_unicode_name_resolves() {
    let fx = fixture(&[(
        "caf.py",
        "def aide():\n    return 1\n\n\ndef caf\u{e9}():\n    return aide()\n",
    )]);
    for (engine, answer) in ask_both(&fx, "caf\u{e9}", 8_000) {
        let answer = answer.unwrap();
        assert!(available(&answer), "{engine}: {}", reason(&answer));
        assert!(
            answer.items.iter().any(|e| e.target_symbol.ends_with("aide")),
            "{engine}: {:?}",
            answer.items
        );
    }
}

/// Several namesakes: refused with the candidates listed, as `impact` refuses
/// them; `file::symbol` picks one.
#[test]
fn an_ambiguous_name_lists_its_candidates() {
    let fx = fixture(&[
        ("a.py", "def target():\n    return 1\n\n\ndef dup():\n    return target()\n"),
        ("b.py", "def target():\n    return 2\n\n\ndef dup():\n    return target()\n"),
    ]);
    let [(_, store), (_, memory)] = ask_both(&fx, "dup", 8_000);
    let store_error = store.expect_err("the store engine refuses an ambiguous name");
    assert!(
        store_error.contains("ambiguous") && store_error.contains("a.py::dup") && store_error.contains("b.py::dup"),
        "{store_error}"
    );
    let memory = memory.unwrap();
    assert!(!available(&memory));
    let memory_reason = reason(&memory);
    assert!(memory_reason.contains("ambiguous") && memory_reason.contains("a.py::dup"), "{memory_reason}");
    for (engine, answer) in ask_both(&fx, "a.py::dup", 8_000) {
        let answer = answer.unwrap();
        assert!(available(&answer), "{engine}: {}", reason(&answer));
        assert!(
            answer.items.iter().all(|e| e.source_file == "a.py"),
            "{engine}: {:?}",
            answer.items
        );
        assert!(!answer.items.is_empty(), "{engine}");
    }
}
