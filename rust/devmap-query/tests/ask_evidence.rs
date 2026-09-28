//! `ask_evidence` is `ask`, regrouped: the same hits in the same order for
//! the hits' share of the budget, plus the file role, the call edges between
//! hits, and the test files that reach them.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{
    EvidenceRole, StoreQueryEngine, ASK_DEFAULT_MIN_CONFIDENCE, EVIDENCE_TEST_BUDGET_SHARE,
};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
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

fn fixture() -> Store {
    store_of(&[
        (
            "cache.py",
            "def cache_lookup(key):\n    return cache_store(key)\n\n\
             def cache_store(key):\n    return key\n",
        ),
        (
            "tests/test_cache.py",
            "from cache import cache_lookup\n\n\
             def test_cache_lookup():\n    assert cache_lookup(1) == 1\n",
        ),
        ("other.py", "def unrelated():\n    return 0\n"),
    ])
}

#[test]
fn the_pack_holds_exactly_the_ask_hits() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);
    let hits_budget = 10_000 - 10_000 / EVIDENCE_TEST_BUDGET_SHARE;
    let ask = engine
        .ask("cache lookup", hits_budget, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    let pack = engine
        .ask_evidence("cache lookup", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert!(!ask.items.is_empty(), "fixture must seed: {ask:?}");
    let mut from_ask: Vec<(String, String)> = ask
        .items
        .iter()
        .map(|hit| (hit.file_path.clone(), hit.symbol_name.clone()))
        .collect();
    let mut from_pack: Vec<(String, String)> = pack
        .files
        .iter()
        .flat_map(|file| file.units.iter())
        .map(|unit| (unit.hit.file_path.clone(), unit.hit.symbol_name.clone()))
        .collect();
    from_ask.sort();
    from_pack.sort();
    assert_eq!(from_ask, from_pack);
    assert_eq!(
        (pack.shown, pack.hidden, pack.total, pack.truncated),
        (ask.shown, ask.hidden, ask.total, ask.truncated)
    );
    // Files appear in the order of their first hit in `ask`.
    let mut first_seen: Vec<&str> = Vec::new();
    for hit in &ask.items {
        if !first_seen.contains(&hit.file_path.as_str()) {
            first_seen.push(&hit.file_path);
        }
    }
    let order: Vec<&str> = pack.files.iter().map(|f| f.file_path.as_str()).collect();
    assert_eq!(order, first_seen);
}

#[test]
fn roles_and_relations_come_from_the_map() {
    let store = fixture();
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("cache", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    let file = |path: &str| {
        pack.files
            .iter()
            .find(|file| file.file_path == path)
            .unwrap_or_else(|| panic!("{path} missing from {pack:#?}"))
    };
    assert_eq!(file("cache.py").role, EvidenceRole::Implementation);
    assert_eq!(file("tests/test_cache.py").role, EvidenceRole::Test);
    assert!(pack.files.iter().all(|f| f.file_path != "other.py"));

    let lookup = file("cache.py")
        .units
        .iter()
        .find(|unit| unit.hit.symbol_name == "cache_lookup")
        .expect("cache_lookup is a hit");
    assert!(
        lookup
            .calls
            .iter()
            .any(|name| name.ends_with("cache_store")),
        "the resolved call between two hits is shown: {lookup:#?}"
    );
    assert!(
        lookup
            .called_by
            .iter()
            .any(|name| name.ends_with("test_cache_lookup")),
        "the test that calls the hit is shown as its caller: {lookup:#?}"
    );
    // The in-memory store has no files on disk, so the source is unavailable
    // and says so; nothing may fold into a unit that shows no text.
    assert!(
        lookup.hit.source_unavailable_reason.is_some(),
        "{lookup:#?}"
    );
    assert!(pack
        .files
        .iter()
        .flat_map(|file| file.units.iter())
        .all(|unit| unit.contained_in.is_none()));
}

#[test]
fn an_empty_question_is_an_empty_pack() {
    let store = fixture();
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert!(pack.files.is_empty());
    assert_eq!(pack.total, 0);
}

#[test]
fn a_test_that_reaches_a_hit_indirectly_is_named() {
    let store = fixture();
    // Only `cache_store` matches; the test reaches it through `cache_lookup`.
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("store", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert!(
        pack.files
            .iter()
            .all(|f| f.role == EvidenceRole::Implementation),
        "{pack:#?}"
    );
    let tests = &pack.related_tests.items;
    assert_eq!(tests.len(), 1, "{pack:#?}");
    assert_eq!(tests[0].path, "tests/test_cache.py");
    assert_eq!(tests[0].depth, 2, "test -> cache_lookup -> cache_store");
}

#[test]
fn a_test_file_already_in_the_pack_is_not_repeated() {
    let store = fixture();
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("cache lookup", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    assert!(pack
        .files
        .iter()
        .any(|f| f.file_path == "tests/test_cache.py"));
    assert!(pack.related_tests.items.is_empty(), "{pack:#?}");
    assert_eq!(pack.related_tests.total, 0, "dropped before budgeting");
}

#[test]
fn the_pack_never_exceeds_its_budget() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);
    for budget in [0, 1, 20, 40, 80, 120, 200, 400, 2_000] {
        let pack = engine
            .ask_evidence("cache", budget, ASK_DEFAULT_MIN_CONFIDENCE)
            .unwrap();
        assert!(
            pack.tokens_used <= budget,
            "budget {budget}: used {}",
            pack.tokens_used
        );
        assert_eq!(pack.shown + pack.hidden, pack.total);
    }
}

#[test]
fn a_test_beside_the_code_it_tests_is_a_test() {
    let store = store_of(&[(
        "ledger.py",
        "def ledger_total(rows):\n    return sum(rows)\n\n\n\
         def test_ledger_total():\n    assert ledger_total([1]) == 1\n",
    )]);
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger total", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    let file = &pack.files[0];
    assert_eq!(file.role, EvidenceRole::Implementation, "{pack:#?}");
    let role = |name: &str| {
        file.units
            .iter()
            .find(|unit| unit.hit.symbol_name == name)
            .unwrap_or_else(|| panic!("{name} missing: {pack:#?}"))
            .role
    };
    assert_eq!(role("ledger_total"), EvidenceRole::Implementation);
    assert_eq!(role("test_ledger_total"), EvidenceRole::Test);
}

#[test]
fn an_inline_test_that_is_not_a_hit_is_a_related_test() {
    // `checks.py` is not a test path and shares no term with the question:
    // only the runner-invoked `test_sums` makes it a test.
    let store = store_of(&[
        (
            "ledger.py",
            "def ledger_total(rows):\n    return sum(rows)\n",
        ),
        (
            "checks.py",
            "from ledger import ledger_total\n\n\n\
             def test_sums():\n    assert ledger_total([1]) == 1\n",
        ),
    ]);
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger total", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
        .unwrap();
    let hits: Vec<&str> = pack
        .files
        .iter()
        .flat_map(|f| &f.units)
        .map(|u| u.hit.symbol_name.as_str())
        .collect();
    assert!(!hits.contains(&"test_sums"), "{hits:?}");
    let tests = &pack.related_tests.items;
    assert_eq!(tests.len(), 1, "{pack:#?}");
    assert_eq!(tests[0].path, "checks.py");
    assert_eq!(tests[0].depth, 1);
    assert!(tests[0].symbols.iter().any(|s| s.ends_with("test_sums")));
}
