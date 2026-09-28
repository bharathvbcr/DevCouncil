//! `ask_evidence` is `ask`, regrouped: same hits, same order, same budget,
//! plus the file role and the call edges between hits.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{EvidenceRole, StoreQueryEngine, ASK_DEFAULT_MIN_CONFIDENCE};
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
    let ask = engine
        .ask("cache lookup", 10_000, ASK_DEFAULT_MIN_CONFIDENCE)
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
