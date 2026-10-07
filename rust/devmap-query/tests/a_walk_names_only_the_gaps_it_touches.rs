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
    let reason = impact
        .walk_incomplete
        .expect("`widget.middle()` names `middle`");
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
    let reason = deps
        .walk_incomplete
        .expect("`other.py` holds an unbound call");
    assert!(reason.contains("thing.frobnicate"), "{reason}");
}

/// Method calls on an untyped receiver whose method name no indexed symbol
/// carries — the shape of `rows.length`, `mu.Unlock()`, `now.Sub(t)`. No edge
/// into this index can hide behind one, because an edge needs a target and no
/// target has that name. Measured on ScholarLM 2026-10-07: 126,023 of the
/// 200,790 `uninferred_receiver` rows, and they put a note on 17 of 21 traces.
const ROWS: &str = "def tally(rows):
    rows.count_everything()
    return rows.sum_the_lot()
";

fn ledger_classes(store: &Store, callee: &str) -> Vec<String> {
    let generation = store.latest_generation_id().unwrap().unwrap();
    let found = store
        .unresolved_sites_naming(generation, &[callee.to_string()], 10)
        .unwrap()
        .unwrap();
    found[callee]
        .0
        .iter()
        .map(|row| row.classification.clone())
        .collect()
}

#[test]
fn a_method_no_symbol_is_named_cannot_hide_a_callee() {
    let store = store_with(&[("app.py", APP), ("rows.py", ROWS)], |_| {});
    // The premise: the sites are in the ledger, unattributed — the class the
    // walk would otherwise have counted.
    assert_eq!(
        ledger_classes(&store, "count_everything"),
        vec!["uninferred_receiver".to_string()]
    );
    let engine = StoreQueryEngine::new(&store);
    let trace = engine.trace(request("rows.py::tally", 3)).unwrap();
    assert_eq!(trace.walk_incomplete, None, "{trace:?}");
    let deps = engine.dependencies(request("rows.py", 1)).unwrap();
    assert_eq!(deps.walk_incomplete, None, "{deps:?}");
}

#[test]
fn the_in_memory_engine_applies_the_same_namesake_test() {
    // Both engines phrase one note through `radius_note`; they must also agree
    // on which sites reach it.
    let extractions = vec![extract_file("app.py", APP), extract_file("rows.py", ROWS)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let engine = devmap_query::QueryEngine::new(&extractions, &resolution);
    let trace = engine.trace(request("rows.py::tally", 3));
    assert_eq!(trace.walk_incomplete, None, "{trace:?}");
}

#[test]
fn sites_that_cannot_hide_an_edge_do_not_use_up_the_per_key_cap() {
    // Five namesake-less calls ahead of one that may hide an edge, in one
    // symbol. The ledger read caps each key at four rows: if the namesake
    // test ran after the cap, the four kept rows would all be filtered out
    // and the one real site would vanish with a "more" flag and no note.
    let source = "class Square:
    def frobnicate(self):
        return 1


class Circle:
    def frobnicate(self):
        return 2


def busy(rows, thing):
    rows.alpha_nothing()
    rows.beta_nothing()
    rows.gamma_nothing()
    rows.delta_nothing()
    rows.epsilon_nothing()
    return thing.frobnicate()
";
    let store = store_with(&[("app.py", APP), ("busy.py", source)], |_| {});
    let trace = StoreQueryEngine::new(&store)
        .trace(request("busy.py::busy", 3))
        .unwrap();
    let reason = trace
        .walk_incomplete
        .clone()
        .unwrap_or_else(|| panic!("`thing.frobnicate()` may hide an edge: {trace:?}"));
    assert!(reason.contains("thing.frobnicate"), "{reason}");
    assert!(reason.starts_with("1 call site(s)"), "{reason}");
    assert!(!reason.contains("_nothing"), "{reason}");
}

#[test]
fn a_bare_local_call_still_counts_without_a_namesake() {
    // `f = make(); f()` — the local may hold any function, so the missing
    // namesake proves nothing about where the call lands.
    let source = "def make():
    return len


def run():
    f = make()
    return f()
";
    let store = store_with(&[("app.py", APP), ("local.py", source)], |_| {});
    let classes = ledger_classes(&store, "f");
    assert!(
        classes.iter().any(|class| class == "local_binding"),
        "the premise: `f()` is an unbound local call: {classes:?}"
    );
    let trace = StoreQueryEngine::new(&store)
        .trace(request("local.py::run", 3))
        .unwrap();
    let reason = trace
        .walk_incomplete
        .clone()
        .unwrap_or_else(|| panic!("`f()` may call anything: {trace:?}"));
    assert!(reason.contains("could not be bound (f)"), "{reason}");
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
