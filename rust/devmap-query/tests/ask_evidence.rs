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
fn without_source_to_fold_the_pack_holds_exactly_the_ask_hits() {
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

// ---------------------------------------------------------------------------
// Against real files. The fixtures above are in-memory, so every source span
// is unavailable and folding never runs; these index a directory on disk.
// ---------------------------------------------------------------------------

use devmap_query::EvidencePack;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-evidence-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn disk_store(root: &Path, files: &[(&str, &str)]) -> Store {
    let mut extractions = Vec::new();
    for (path, body) in files {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, body).unwrap();
        extractions.push(extract_file(path, body));
    }
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
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store
}

const LEDGER: &str = "class Ledger:\n    \"\"\"Ledger totals.\"\"\"\n\n    \
def ledger_total(self, rows):\n        return sum(rows)\n\n    \
def ledger_reset(self):\n        return 0\n\n\n\
def ledger_report(rows):\n    return Ledger().ledger_total(rows)\n";
const LEDGER_TEST: &str = "from ledger import ledger_report\n\n\n\
def test_report():\n    assert ledger_report([1]) == 1\n";

/// The invariants every pack must hold, whatever the input.
fn assert_pack_is_sound(pack: &EvidencePack, budget: u32) {
    assert!(
        pack.tokens_used <= budget,
        "used {} of {budget}",
        pack.tokens_used
    );
    assert_eq!(pack.shown + pack.hidden, pack.total, "hit counters");
    let tests = &pack.related_tests;
    assert_eq!(tests.shown + tests.hidden, tests.total, "test counters");
    assert_eq!(tests.shown as usize, tests.items.len());
    let units: Vec<_> = pack.files.iter().flat_map(|f| &f.units).collect();
    assert_eq!(
        units.len(),
        pack.shown as usize,
        "every shown hit is a unit"
    );
    for file in &pack.files {
        assert!(file.units.iter().all(|u| u.hit.file_path == file.file_path));
    }
    for unit in &units {
        let Some(container) = &unit.contained_in else {
            continue;
        };
        assert!(
            unit.hit.source_span.is_empty(),
            "a folded unit prints nothing"
        );
        let shown = units
            .iter()
            .find(|other| &other.qualified_name == container)
            .unwrap_or_else(|| panic!("{container} is not in the pack"));
        assert!(
            shown.contained_in.is_none()
                && !shown.hit.source_span.is_empty()
                && shown.hit.source_unavailable_reason.is_none()
                && shown.hit.source_span_omitted_bytes.is_none(),
            "{} folds into {container}, which does not print its whole source: {shown:#?}",
            unit.qualified_name
        );
        assert_eq!(shown.hit.file_path, unit.hit.file_path);
        assert!(shown.hit.span.0 <= unit.hit.span.0 && unit.hit.span.1 <= shown.hit.span.1);
    }
}

#[test]
fn folded_source_is_really_printed_by_its_container() {
    let root = scratch("fold");
    let store = disk_store(
        &root,
        &[("ledger.py", LEDGER), ("tests/test_ledger.py", LEDGER_TEST)],
    );
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger total", 10_000, 0.0)
        .unwrap();
    assert_pack_is_sound(&pack, 10_000);
    let units: Vec<_> = pack.files.iter().flat_map(|f| &f.units).collect();
    let folded: Vec<_> = units.iter().filter(|u| u.contained_in.is_some()).collect();
    assert!(
        !folded.is_empty(),
        "the methods sit inside the class: {pack:#?}"
    );
    for unit in folded {
        let container = units
            .iter()
            .find(|u| Some(&u.qualified_name) == unit.contained_in.as_ref())
            .unwrap();
        assert!(
            container.hit.source_span.contains(&unit.hit.symbol_name),
            "{} is not in the text of {}",
            unit.hit.symbol_name,
            container.qualified_name
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_edited_file_is_never_a_container() {
    let root = scratch("edited");
    let store = disk_store(
        &root,
        &[("ledger.py", LEDGER), ("tests/test_ledger.py", LEDGER_TEST)],
    );
    // Same length, different bytes: the stored hash no longer matches.
    std::fs::write(root.join("ledger.py"), LEDGER.replace("sum", "max")).unwrap();
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger total", 10_000, 0.0)
        .unwrap();
    assert_pack_is_sound(&pack, 10_000);
    let file = pack
        .files
        .iter()
        .find(|f| f.file_path == "ledger.py")
        .expect("the hits stay as leads");
    for unit in &file.units {
        assert!(unit.contained_in.is_none(), "{unit:#?}");
        assert!(
            unit.hit.source_span.is_empty(),
            "stale text must not be shown"
        );
        assert!(
            unit.hit.source_unavailable_reason.is_some(),
            "and it must say why: {unit:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_deleted_file_is_named_not_dropped() {
    let root = scratch("deleted");
    let store = disk_store(
        &root,
        &[("ledger.py", LEDGER), ("tests/test_ledger.py", LEDGER_TEST)],
    );
    std::fs::remove_file(root.join("ledger.py")).unwrap();
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger total", 10_000, 0.0)
        .unwrap();
    assert_pack_is_sound(&pack, 10_000);
    assert!(pack.files.iter().any(|f| f.file_path == "ledger.py"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn every_budget_keeps_every_contract() {
    let root = scratch("budgets");
    let store = disk_store(
        &root,
        &[("ledger.py", LEDGER), ("tests/test_ledger.py", LEDGER_TEST)],
    );
    let engine = StoreQueryEngine::new(&store);
    for budget in [
        0, 1, 3, 4, 19, 20, 21, 39, 40, 60, 80, 100, 150, 250, 500, 2_000, 100_000,
    ] {
        for floor in [0.0, ASK_DEFAULT_MIN_CONFIDENCE] {
            let pack = engine.ask_evidence("ledger", budget, floor).unwrap();
            assert_pack_is_sound(&pack, budget);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn hostile_questions_are_answered_or_refused_never_panicked_on() {
    let root = scratch("hostile");
    let store = disk_store(&root, &[("ledger.py", LEDGER)]);
    let engine = StoreQueryEngine::new(&store);
    let long = "ledger ".repeat(600);
    for query in [
        "",
        "   ",
        "!!!???",
        "\u{0}\u{1b}[31m",
        "ledger\u{202e}total",
        "lédger tötal 帳簿",
        long.as_str(),
    ] {
        let pack = engine.ask_evidence(query, 2_000, 0.0).unwrap();
        assert_pack_is_sound(&pack, 2_000);
    }
    // The engine refuses only what no comparison can evaluate; the range is
    // the CLI's and the IPC/MCP schema's to enforce, and they do.
    assert!(engine.ask_evidence("ledger", 2_000, f32::NAN).is_err());
    for floor in [-0.5, 1.5, f32::INFINITY, f32::NEG_INFINITY] {
        let pack = engine.ask_evidence("ledger", 2_000, floor).unwrap();
        assert_pack_is_sound(&pack, 2_000);
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Folding buys room: a method inside a shown class costs its lead, not its
/// text, so for the same hit budget the pack holds every hit `ask` would —
/// in the same order — and sometimes more.
#[test]
fn folding_makes_room_for_more_hits_and_never_fewer() {
    let root = scratch("room");
    let store = disk_store(
        &root,
        &[("ledger.py", LEDGER), ("tests/test_ledger.py", LEDGER_TEST)],
    );
    let engine = StoreQueryEngine::new(&store);
    let mut gained = false;
    for budget in (40..=800).step_by(8) {
        let hits_budget = budget - budget / EVIDENCE_TEST_BUDGET_SHARE;
        let ask = engine.ask("ledger", hits_budget, 0.0).unwrap();
        let pack = engine.ask_evidence("ledger", budget, 0.0).unwrap();
        assert_pack_is_sound(&pack, budget);
        assert!(
            pack.shown >= ask.shown,
            "budget {budget}: {} < {}",
            pack.shown,
            ask.shown
        );
        let in_pack: Vec<(&str, &str)> = pack
            .files
            .iter()
            .flat_map(|f| &f.units)
            .map(|u| (u.hit.file_path.as_str(), u.hit.symbol_name.as_str()))
            .collect();
        for hit in &ask.items {
            assert!(
                in_pack.contains(&(hit.file_path.as_str(), hit.symbol_name.as_str())),
                "budget {budget}: {} dropped",
                hit.symbol_name
            );
        }
        gained |= pack.shown > ask.shown;
    }
    assert!(gained, "no budget let folding admit an extra hit");
    let _ = std::fs::remove_dir_all(root);
}

/// A whole-file hit whose text had to be capped is a lead, not a block.
///
/// Its capped text is the file's first few lines, and a capped span cannot
/// stand in for anything, so printing it spent up to a quarter of the budget
/// on imports. The pack names the file, says how many bytes it did not show,
/// and charges only the lead — the rest of the budget goes to the hits.
#[test]
fn a_capped_whole_file_hit_is_a_lead_not_a_block() {
    let root = scratch("capped-file");
    let mut body = String::new();
    for index in 0..120 {
        body.push_str(&format!(
            "def ledger_step_{index:03}(rows):\n    \"\"\"Step {index} of the ledger.\"\"\"\n    return rows\n\n\n"
        ));
    }
    let store = disk_store(&root, &[("ledger.py", &body)]);
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger", 2_000, 0.0)
        .unwrap();
    assert_pack_is_sound(&pack, 2_000);
    let file_unit = pack
        .files
        .iter()
        .flat_map(|f| &f.units)
        .find(|u| u.hit.kind.eq_ignore_ascii_case("file"))
        .unwrap_or_else(|| panic!("the file symbol matches `ledger`: {pack:#?}"));
    assert!(file_unit.hit.source_span.is_empty(), "{file_unit:#?}");
    assert_eq!(
        file_unit.hit.source_span_omitted_bytes,
        Some(body.len() as u32),
        "the whole file is reported as not shown"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A method's first line gets its indentation back; a top-level symbol has
/// none to give and adds no key to the wire form.
#[test]
fn a_nested_symbol_carries_the_indentation_its_first_line_lost() {
    let root = scratch("indent");
    let store = disk_store(&root, &[("ledger.py", LEDGER)]);
    let hits = StoreQueryEngine::new(&store)
        .ask("ledger", 10_000, 0.0)
        .unwrap()
        .items;
    let method = hits
        .iter()
        .find(|hit| hit.symbol_name == "ledger_total")
        .expect("the method is a hit");
    assert_eq!(method.source_indent.as_deref(), Some("    "), "{method:#?}");
    assert!(method.source_span.starts_with("def ledger_total"));
    let function = hits
        .iter()
        .find(|hit| hit.symbol_name == "ledger_report")
        .expect("the function is a hit");
    assert_eq!(function.source_indent, None);
    let wire = serde_json::to_value(function).unwrap();
    assert!(wire.get("source_indent").is_none(), "{wire}");
    let _ = std::fs::remove_dir_all(root);
}

/// The repository-wide attribution gap is stated once, on the pack; the
/// related-test list says only where its own walk stopped.
#[test]
fn the_repository_wide_gap_is_stated_once() {
    let root = scratch("gap");
    let store = disk_store(
        &root,
        &[
            (
                "ledger.py",
                // `rows` has no inferable type and two classes define
                // `frobnicate`, so the call has namesakes but no target: an
                // unexplained attribution site, which is what the gap counts.
                "def ledger_total(rows):\n    return rows.frobnicate()\n",
            ),
            (
                "shapes.py",
                "class Square:\n    def frobnicate(self):\n        return 1\n\n\n\
                 class Circle:\n    def frobnicate(self):\n        return 2\n",
            ),
            (
                "tests/test_ledger.py",
                "from ledger import ledger_total\n\n\n\
                 def test_total():\n    assert ledger_total([1]) == 1\n",
            ),
        ],
    );
    let pack = StoreQueryEngine::new(&store)
        .ask_evidence("ledger total", 10_000, 0.0)
        .unwrap();
    assert_pack_is_sound(&pack, 10_000);
    let gap = pack
        .coverage_gap
        .as_deref()
        .unwrap_or_else(|| panic!("an unresolved call is a coverage gap: {pack:#?}"));
    assert!(gap.contains("repository-wide"), "{gap}");
    let list = pack.related_tests.walk_incomplete.as_deref().unwrap_or("");
    assert!(
        !list.contains("repository-wide") && !list.contains(gap),
        "the list repeats the gap: {list}"
    );
    let _ = std::fs::remove_dir_all(root);
}
