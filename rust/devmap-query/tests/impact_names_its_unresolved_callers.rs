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
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
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
        (
            "rare.py",
            "def rare(book):\n    return book.close()\n".to_string(),
        ),
    ]);
    let generation = store.latest_generation_id().unwrap().unwrap();
    let names = vec![
        "settle".to_string(),
        "close".to_string(),
        "absent".to_string(),
    ];
    let found = store
        .unresolved_sites_naming(generation, &names, MAX_NAMESAKE_SITES_PER_NAME)
        .unwrap()
        .expect("the generation is retained");
    assert_eq!(found["settle"].0.len(), MAX_NAMESAKE_SITES_PER_NAME);
    assert!(found["settle"].1, "the generic name hit its cap");
    assert_eq!(found["close"].0.len(), 1);
    assert!(!found["close"].1, "the rare name was not cut");
    assert!(
        found["absent"].0.is_empty() && !found["absent"].1,
        "a checked absence"
    );
    let files: Vec<&str> = found["settle"]
        .0
        .iter()
        .map(|row| row.source_symbol.as_str())
        .collect();
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(
        files, sorted,
        "a capped prefix is the same prefix on every read"
    );
}

/// The ledger is read at the walk's generation. A site that only a later
/// generation holds must not appear in an answer about the earlier one, and a
/// generation that is gone answers None rather than silently reading another.
#[test]
fn the_ledger_is_read_at_the_generation_asked_for() {
    let store = store_of(&[("ledger.py", LEDGER.to_string())]);
    let first = store.latest_generation_id().unwrap().unwrap();
    save(
        &store,
        &[
            ("ledger.py", LEDGER.to_string()),
            ("untyped.py", UNTYPED.to_string()),
        ],
    );
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
    assert_eq!(
        at(second + 1000),
        None,
        "a generation that is not retained is not read"
    );
}

/// `callers` untyped `book.settle()` call sites in one file, beside the
/// `Ledger.settle` they name. Every one lands in the ledger, none is an edge.
fn many_untyped_callers(callers: usize) -> Store {
    let many: String = (0..callers)
        .map(|index| format!("def caller_{index}(book):\n    return book.settle()\n\n\n"))
        .collect();
    store_of(&[("ledger.py", LEDGER.to_string()), ("many.py", many)])
}

/// `sites_omitted` as the wire carries it: absent means none were trimmed.
fn sites_omitted(namesakes: &devmap_query::UnresolvedNamesakes) -> usize {
    serde_json::to_value(namesakes)
        .unwrap()
        .get("sites_omitted")
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |omitted| usize::try_from(omitted).unwrap())
}

fn req_at(target: &str, token_budget: u32) -> Request<String> {
    Request {
        token_budget,
        ..req(target)
    }
}

/// The ledger's candidates are advisory data, and they were the one part of an
/// `impact` answer no budget touched: 400 sites could ride on a 1,500-token
/// call. They are charged to a share of the caller's budget now, and the sites
/// the share could not hold are counted, not dropped.
#[test]
fn candidates_beyond_the_budget_share_are_counted_not_listed() {
    let store = many_untyped_callers(40);
    let engine = StoreQueryEngine::new(&store);

    let full = engine.impact(req_at("Ledger.settle", 100_000)).unwrap();
    let full = full.unresolved_namesakes.expect("checked");
    assert_eq!(
        full.sites.len(),
        40,
        "the premise: all 40 are in the ledger"
    );
    assert_eq!(
        sites_omitted(&full),
        0,
        "a budget that holds them trims none"
    );

    let tight = engine.impact(req_at("Ledger.settle", 1_500)).unwrap();
    let note = tight.walk_incomplete.clone().unwrap_or_default();
    let tight = tight.unresolved_namesakes.expect("checked");
    assert!(
        !tight.sites.is_empty() && tight.sites.len() < 40,
        "a quarter of 1,500 tokens holds some of 40 sites, not all and not none: {}",
        tight.sites.len()
    );
    assert_eq!(
        tight.sites.len() + sites_omitted(&tight),
        full.sites.len(),
        "listed plus omitted must account for every site the ledger held"
    );
    assert_eq!(
        tight.sites[..],
        full.sites[..tight.sites.len()],
        "the listed sites are a prefix of the full read, in the same order"
    );
    assert!(!tight.truncated, "no per-name cap bit; only the budget did");
    assert!(
        note.contains("40 unresolved call site(s) name settle"),
        "walk_incomplete must name what the ledger held, not what fitted: {note:?}"
    );
}

/// The per-name cap and the budget share are two different signals and both
/// stay honest when both bite.
#[test]
fn a_capped_ledger_read_trimmed_by_budget_keeps_both_signals() {
    let store = many_untyped_callers(MAX_NAMESAKE_SITES_PER_NAME + 10);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine.impact(req_at("Ledger.settle", 1_500)).unwrap();
    let note = answer.walk_incomplete.clone().unwrap_or_default();
    let namesakes = answer.unresolved_namesakes.expect("checked");

    assert!(namesakes.truncated, "the per-name cap bit");
    assert!(sites_omitted(&namesakes) > 0, "and so did the budget");
    assert_eq!(
        namesakes.sites.len() + sites_omitted(&namesakes),
        MAX_NAMESAKE_SITES_PER_NAME
    );
    assert!(
        note.contains(&format!("at least {MAX_NAMESAKE_SITES_PER_NAME}")),
        "{note:?}"
    );
}

/// A namesake-dominated answer is no longer several times its budget.
#[test]
fn a_namesake_dominated_answer_stays_near_its_budget() {
    let store = many_untyped_callers(40);
    let engine = StoreQueryEngine::new(&store);
    let budget = 600u32;

    let answer = engine.impact(req_at("Ledger.settle", budget)).unwrap();
    let bytes = serde_json::to_string(&answer).unwrap().len();

    let allowed = budget as usize * devmap_query::BYTES_PER_TOKEN as usize * 3 / 2;
    assert!(
        bytes <= allowed,
        "{bytes} bytes for a {budget}-token budget (allowed {allowed})"
    );
}

/// Sites that fit are not trimmed, and the wire form gains no key.
#[test]
fn candidates_that_fit_keep_their_wire_form() {
    let store = store_of(&[
        ("ledger.py", LEDGER.to_string()),
        ("untyped.py", UNTYPED.to_string()),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let wire =
        serde_json::to_value(engine.impact(req_at("Ledger.settle", 1_500)).unwrap()).unwrap();
    let namesakes = &wire["unresolved_namesakes"];
    assert_eq!(namesakes["sites"].as_array().map(Vec::len), Some(1));
    assert!(
        namesakes.get("sites_omitted").is_none(),
        "nothing was trimmed, so nothing is reported: {namesakes}"
    );
}
