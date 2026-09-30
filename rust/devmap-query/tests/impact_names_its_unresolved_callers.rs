//! `impact` reports the unresolved call sites that name its target.
//!
//! An edge walk answers "who calls this?" from edges, and a caller the resolver
//! could not bind is not an edge. It is in the ledger, by name, with the reason
//! it was not bound — and no query read it. Three user reports on one day were
//! this shape: scholarlm's `recordReplayEvent` and `replayEventsAfter`, reached
//! through `job` from an untyped return, and BINN's figure scripts, reached
//! through a module loaded by file path. Each time the agent was shown a subset
//! of the callers with nothing saying so, and found the rest with `rg`.
//!
//! These tests pin that the candidates are reported, that they are bounded,
//! that they stay in the target's language, and that `walk_incomplete` says
//! the edge list is not the whole answer when they exist.

use devmap_extract::extract_file;
use devmap_query::{Request, StoreQueryEngine, MAX_NAMESAKE_SITES_PER_NAME};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn store_of(files: &[(&str, String)]) -> Store {
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
    store
}

fn req(target: &str) -> Request<String> {
    Request {
        query: target.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth: 3,
    }
}

const LEDGER: &str = "class Ledger:\n    def settle(self):\n        return 1\n\n\ndef make():\n    return Ledger()\n";
/// A caller the resolver binds: the receiver is constructed in scope.
const TYPED: &str =
    "from ledger import Ledger\n\n\ndef typed():\n    book = Ledger()\n    return book.settle()\n";
/// A caller it cannot: the receiver is a parameter with no annotation.
const UNTYPED: &str = "def untyped(book):\n    return book.settle()\n";

#[test]
fn an_untyped_caller_is_reported_beside_the_edges() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("typed.py", TYPED.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let answer = engine.impact(req("Ledger.settle")).unwrap();

    assert!(
        answer
            .items
            .iter()
            .any(|edge| edge.source_symbol.ends_with("typed")),
        "the bound caller is an edge: {:?}",
        answer.items
    );
    let namesakes = answer
        .unresolved_namesakes
        .as_ref()
        .expect("a symbol target always states its ledger check");
    assert_eq!(namesakes.names, vec!["settle".to_string()]);
    let sites: Vec<(&str, Option<&str>)> = namesakes
        .sites
        .iter()
        .map(|site| (site.source_symbol.as_str(), site.receiver.as_deref()))
        .collect();
    assert_eq!(sites, vec![("untyped.py::untyped", Some("book"))]);
    assert!(!namesakes.truncated);
    let note = answer.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        note.contains("1 unresolved call site(s) name settle"),
        "the edge list must not read as the whole answer: {note:?}"
    );
}

/// The blind case at its sharpest: every caller is unresolved, so the walk
/// finds no call edge into the target at all. The ledger still has them.
#[test]
fn a_target_with_no_bound_caller_still_gets_its_candidates() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let answer = engine.impact(req("ledger.py::Ledger.settle")).unwrap();
    assert!(
        !answer
            .items
            .iter()
            .any(|edge| edge.source_symbol.ends_with("untyped")),
        "the premise: the untyped caller is not an edge: {:?}",
        answer.items
    );
    let namesakes = answer
        .unresolved_namesakes
        .expect("named by the query itself");
    assert_eq!(namesakes.sites.len(), 1);
    assert_eq!(namesakes.sites[0].source_file, "untyped.py");
    assert!(answer
        .walk_incomplete
        .as_deref()
        .is_some_and(|note| note.contains("unresolved_namesakes")));
}

#[test]
fn a_checked_absence_is_an_empty_list_and_adds_no_warning() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("typed.py", TYPED.to_string()),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .impact(req("Ledger.settle"))
        .unwrap();
    let namesakes = answer.unresolved_namesakes.expect("checked");
    assert!(namesakes.sites.is_empty() && !namesakes.truncated);
    assert!(
        !answer
            .walk_incomplete
            .as_deref()
            .unwrap_or_default()
            .contains("unresolved_namesakes"),
        "nothing found in the ledger is not a reason to call the answer partial"
    );
}

#[test]
fn another_languages_namesake_is_counted_not_listed() {
    let go = "package pay\n\nfunc drain(x any) {\n\tv := load(x)\n\tv.Settle()\n\tv.settle()\n}\n";
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("untyped.py", UNTYPED.to_string()),
        ("pay/drain.go", go.to_string()),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .impact(req("ledger.py::Ledger.settle"))
        .unwrap();
    let namesakes = answer.unresolved_namesakes.unwrap();
    assert_eq!(namesakes.sites.len(), 1, "{:?}", namesakes.sites);
    assert_eq!(namesakes.sites[0].source_file, "untyped.py");
    assert_eq!(namesakes.other_language_sites, 1, "the Go `v.settle()`");
}

#[test]
fn a_generic_name_is_capped_and_says_so() {
    let many: String = (0..MAX_NAMESAKE_SITES_PER_NAME + 10)
        .map(|index| format!("def caller_{index}(book):\n    return book.settle()\n\n\n"))
        .collect();
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("typed.py", TYPED.to_string()),
        ("many.py", many),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .impact(req("Ledger.settle"))
        .unwrap();
    let namesakes = answer.unresolved_namesakes.unwrap();
    assert_eq!(namesakes.sites.len(), MAX_NAMESAKE_SITES_PER_NAME);
    assert!(namesakes.truncated);
    assert!(answer
        .walk_incomplete
        .as_deref()
        .is_some_and(|note| note.contains(&format!("at least {MAX_NAMESAKE_SITES_PER_NAME}"))));
}

#[test]
fn a_file_target_checks_the_names_of_its_members() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("typed.py", TYPED.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .impact(req("ledger.py"))
        .unwrap();
    let namesakes = answer
        .unresolved_namesakes
        .expect("a file's impact is its members' impact, callers by name included");
    assert!(
        namesakes.names.contains(&"settle".to_string()),
        "{:?}",
        namesakes.names
    );
    assert!(namesakes
        .sites
        .iter()
        .any(|site| site.source_file == "untyped.py"));
}

#[test]
fn a_file_with_more_members_than_the_name_cap_says_it_did_not_look() {
    let wide: String = (0..devmap_query::MAX_NAMESAKE_NAMES + 3)
        .map(|index| format!("def member_{index}():\n    return {index}\n\n\n"))
        .collect();
    let callers: String = (0..devmap_query::MAX_NAMESAKE_NAMES + 3)
        .map(|index| format!("from wide import member_{index}\n"))
        .chain(std::iter::once("\n\ndef call_all():\n".to_string()))
        .chain(
            (0..devmap_query::MAX_NAMESAKE_NAMES + 3)
                .map(|index| format!("    member_{index}()\n")),
        )
        .collect();
    let store = store_of(&[("wide.py", wide), ("callers.py", callers)]);
    let answer = StoreQueryEngine::new(&store)
        .impact(req("wide.py"))
        .unwrap();
    let namesakes = answer.unresolved_namesakes.as_ref().unwrap();
    assert_eq!(namesakes.names.len(), devmap_query::MAX_NAMESAKE_NAMES);
    assert_eq!(namesakes.names_not_checked, 3);
    assert!(
        answer
            .walk_incomplete
            .as_deref()
            .is_some_and(|note| note.contains("3 more were not looked up")),
        "{:?}",
        answer.walk_incomplete
    );
}

#[test]
fn a_blank_target_checks_nothing() {
    let store = store_of(&[("ledger.py", LEDGER.to_string())]);
    let answer = StoreQueryEngine::new(&store).impact(req("   ")).unwrap();
    assert!(answer.unresolved_namesakes.is_none());
}

#[test]
fn the_banded_answer_carries_the_same_candidates() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("typed.py", TYPED.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let flat = engine.impact(req("Ledger.settle")).unwrap();
    let layered = engine.impact_layered(req("Ledger.settle")).unwrap();
    assert_eq!(
        flat.unresolved_namesakes,
        layered.edges.unresolved_namesakes
    );
}

#[test]
fn a_forward_trace_does_not_read_the_ledger() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .trace(req("untyped.py::untyped"))
        .unwrap();
    assert!(answer.unresolved_namesakes.is_none());
}

#[test]
fn the_wire_form_omits_the_field_only_when_it_was_not_checked() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let wire = serde_json::to_value(engine.impact(req("Ledger.settle")).unwrap()).unwrap();
    assert_eq!(
        wire["unresolved_namesakes"]["sites"][0]["classification"],
        serde_json::json!("uninferred_receiver")
    );
    let trace = serde_json::to_value(engine.trace(req("untyped.py::untyped")).unwrap()).unwrap();
    assert!(
        trace.get("unresolved_namesakes").is_none(),
        "not checked, so not on the wire"
    );
}

fn save(store: &Store, files: &[(&str, String)]) {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation_with_opts(&extractions, &resolution, &analysis, GenerationWriteOpts::default())
        .unwrap();
}

/// One ledger pass answers every name, each capped on its own: a generic name
/// that fills its cap must not starve, or flag, a rare one read beside it.
#[test]
fn one_read_caps_each_name_separately() {
    let many: String = (0..MAX_NAMESAKE_SITES_PER_NAME + 5)
        .map(|index| format!("def caller_{index}(book):\n    return book.settle()\n\n\n"))
        .collect();
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("many.py", many),
        ("rare.py", "def rare(book):\n    return book.close()\n".to_string()),
    ]);
    let generation = store.latest_generation_id().unwrap().unwrap();
    let names = vec!["settle".to_string(), "close".to_string(), "absent".to_string()];
    let found = store
        .unresolved_sites_naming(generation, &names, MAX_NAMESAKE_SITES_PER_NAME)
        .unwrap()
        .expect("the generation is retained");
    assert_eq!(found["settle"].0.len(), MAX_NAMESAKE_SITES_PER_NAME);
    assert!(found["settle"].1, "the generic name hit its cap");
    assert_eq!(found["close"].0.len(), 1);
    assert!(!found["close"].1, "the rare name was not cut");
    assert!(found["absent"].0.is_empty() && !found["absent"].1, "a checked absence");
    let files: Vec<&str> = found["settle"].0.iter().map(|row| row.source_symbol.as_str()).collect();
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted, "a capped prefix is the same prefix on every read");
}

/// The ledger is read at the walk's generation. A site that only a later
/// generation holds must not appear in an answer about the earlier one, and a
/// generation that is gone answers None rather than silently reading another.
#[test]
fn the_ledger_is_read_at_the_generation_asked_for() {
    let store = store_of(&[("ledger.py", LEDGER.to_string())]);
    let first = store.latest_generation_id().unwrap().unwrap();
    save(&store, &[("ledger.py", LEDGER.to_string()), ("untyped.py", UNTYPED.to_string())]);
    let second = store.latest_generation_id().unwrap().unwrap();
    assert!(second > first);
    let names = vec!["settle".to_string()];
    let at = |generation| {
        store
            .unresolved_sites_naming(generation, &names, 10)
            .unwrap()
            .map(|found| found["settle"].0.len())
    };
    assert_eq!(at(first), Some(0), "the untyped caller did not exist yet");
    assert_eq!(at(second), Some(1));
    assert_eq!(at(second + 1000), None, "a generation that is not retained is not read");
}
