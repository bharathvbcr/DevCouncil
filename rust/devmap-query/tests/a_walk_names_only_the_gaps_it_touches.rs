//! A walk's `walk_incomplete` names the unbound calls that touch *it*, not a
//! repository-wide count every answer carried alike.
//!
//! Measured 2026-10-07 on ScholarLM: every `impact` and `affected` answer said
//! "212,976 of 571,999 unresolved attribution site(s) have no indexed target
//! … these repository-wide counts are not specific to this target". A marker
//! that rides on every answer is one a caller learns to skip, and an agent did:
//! it reported "every graph walk reported itself incomplete, so the lists of
//! affected tests are lower bounds" for walks the ledger had nothing to say
//! about.
//!
//! The corpus below has unbound calls in `other.py` that touch nothing `app.py`
//! reaches. A walk over `app.py` must now answer clean; a walk that reaches a
//! symbol an unbound call names, or walks into a symbol holding one, must name
//! that site.

use devmap_extract::extract_file;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const APP: &str = "def leaf():
    return 1


def middle():
    return leaf()


def top():
    return middle()
";

/// `thing.frobnicate()`: two classes define it and `thing` has no type, so it
/// is an unbound site with namesakes — exactly what may hide an edge. And
/// `widget.middle()` names `middle`, which `impact leaf` reaches at depth 1.
const OTHER: &str = "class Square:
    def frobnicate(self):
        return 1


class Circle:
    def frobnicate(self):
        return 2


def other(thing):
    return thing.frobnicate()


def uses_middle(widget):
    return widget.middle()
";

fn store_with(
    files: &[(&str, &str)],
    edit: impl FnOnce(&mut devmap_analyze::AnalysisSummary),
) -> Store {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let mut analysis = devmap_analyze::analyze(&extractions, &resolution);
    edit(&mut analysis);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn corpus() -> Store {
    store_with(&[("app.py", APP), ("other.py", OTHER)], |_| {})
}

fn request(query: &str, depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 20_000,
        min_confidence: 0.0,
        max_depth: depth,
    }
}

#[test]
fn the_corpus_really_has_an_unbound_call_elsewhere() {
    // The premise every clean answer below depends on: without it, "clean"
    // would be the old behaviour too.
    let store = corpus();
    let analysis = store.latest_analysis().unwrap().unwrap();
    let rate = &analysis.resolution_rate;
    assert!(
        rate.unresolved_sites > rate.explained_sites,
        "the fixture must hold an unexplained site: {rate:?}"
    );
}

#[test]
fn a_walk_untouched_by_the_unbound_calls_answers_clean() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let impact = engine.impact(request("app.py::top", 3)).unwrap();
    assert_eq!(impact.walk_incomplete, None, "{impact:?}");
    let trace = engine.trace(request("app.py::top", 3)).unwrap();
    assert_eq!(trace.walk_incomplete, None, "{trace:?}");
    let deps = engine.dependencies(request("app.py", 1)).unwrap();
    assert_eq!(deps.walk_incomplete, None, "{deps:?}");
}

#[test]
fn a_caller_walk_names_the_unbound_site_that_names_a_reached_symbol() {
    let store = corpus();
    let impact = StoreQueryEngine::new(&store)
        .impact(request("app.py::leaf", 3))
        .unwrap();
    let reason = impact.walk_incomplete.expect("`widget.middle()` names `middle`");
    assert!(reason.contains("widget.middle"), "{reason}");
    assert!(!reason.contains("repository-wide"), "{reason}");
}

#[test]
fn a_callee_walk_names_the_unbound_site_inside_what_it_walked() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let trace = engine.trace(request("other.py::other", 3)).unwrap();
    let reason = trace
        .walk_incomplete
        .clone()
        .unwrap_or_else(|| panic!("`other` holds an unbound call: {trace:?}"));
    assert!(reason.contains("thing.frobnicate"), "{reason}");
    let deps = engine.dependencies(request("other.py", 1)).unwrap();
    let reason = deps.walk_incomplete.expect("`other.py` holds an unbound call");
    assert!(reason.contains("thing.frobnicate"), "{reason}");
}

#[test]
fn affected_tests_drop_the_repository_wide_count() {
    let store = store_with(
        &[
            ("app.py", APP),
            ("other.py", OTHER),
            (
                "tests/test_app.py",
                "from app import top\n\n\ndef test_top():\n    assert top() == 1\n",
            ),
        ],
        |_| {},
    );
    let affected = StoreQueryEngine::new(&store)
        .affected_tests(&["app.py::top".to_string()], 20_000, 0.0, 3)
        .unwrap();
    assert!(
        !affected.tests.items.is_empty(),
        "the premise: `test_top` reaches `top`: {affected:?}"
    );
    assert_eq!(affected.tests.walk_incomplete, None, "{affected:?}");
    assert_eq!(
        affected.blast_radius.layers.walk_incomplete, None,
        "{affected:?}"
    );
}

#[test]
fn a_capped_check_says_it_was_capped() {
    // 600 distinct callers of `leaf`: more reached names than one answer
    // checks against the ledger. A capped check must not read as clean.
    let mut source = String::from("def leaf():\n    return 1\n");
    for index in 0..600 {
        source.push_str(&format!("\n\ndef caller_{index}():\n    return leaf()\n"));
    }
    let store = store_with(&[("app.py", &source), ("other.py", OTHER)], |_| {});
    let impact = StoreQueryEngine::new(&store)
        .impact(request("app.py::leaf", 1))
        .unwrap();
    let reason = impact.walk_incomplete.expect("the check was capped");
    assert!(reason.contains("only 512 of 601"), "{reason}");
}

#[test]
fn an_inconsistent_breakdown_falls_back_to_the_repository_wide_sentence() {
    // The specific check needs the classification breakdown; without it the
    // answer must keep saying coverage is unknown rather than go quiet.
    let store = store_with(&[("app.py", APP), ("other.py", OTHER)], |analysis| {
        analysis.resolution_rate.unresolved_sites = 0;
        analysis.resolution_rate.explained_sites = 0;
    });
    let impact = StoreQueryEngine::new(&store)
        .impact(request("app.py::top", 3))
        .unwrap();
    let reason = impact.walk_incomplete.expect("coverage is unknown");
    assert!(reason.contains("unavailable or inconsistent"), "{reason}");
}
