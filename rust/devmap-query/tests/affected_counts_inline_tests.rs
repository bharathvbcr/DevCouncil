//! `affected_tests` names a test that lives beside the code it tests.
//!
//! A test is a symbol in a test file *or* one a test runner invokes — the
//! extractor records `#[test]`, pytest `test_*` and the like as
//! `RuntimeEntryPoint` wiring. Before this, a `#[cfg(test)] mod tests` in
//! `src/lib.rs` was invisible: its path is not a test path.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::StoreQueryEngine;
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

#[test]
fn a_rust_test_beside_its_code_is_an_affected_test() {
    let store = store_of(&[(
        "src/lib.rs",
        "pub fn total(rows: &[u32]) -> u32 {\n    rows.iter().sum()\n}\n\n\
         pub fn report(rows: &[u32]) -> String {\n    total(rows).to_string()\n}\n\n\
         #[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn sums() {\n        \
         assert_eq!(total(&[1]), 1);\n    }\n}\n",
    )]);
    let answer = StoreQueryEngine::new(&store)
        .affected_tests(&["total".into()], 8_000, 0.0, 3)
        .unwrap();
    let tests = &answer.tests.items;
    assert_eq!(tests.len(), 1, "{answer:#?}");
    assert_eq!(tests[0].path, "src/lib.rs");
    assert!(
        tests[0]
            .symbols
            .iter()
            .all(|symbol| symbol.ends_with("sums")),
        "only the #[test] fn counts, not `report`, which also calls `total`: {:?}",
        tests[0].symbols
    );
}

#[test]
fn a_pytest_function_outside_a_test_path_is_an_affected_test() {
    let store = store_of(&[
        (
            "ledger.py",
            "def ledger_total(rows):\n    return sum(rows)\n",
        ),
        (
            "checks.py",
            "from ledger import ledger_total\n\n\n\
             def test_sums():\n    assert ledger_total([1]) == 1\n\n\n\
             def summary(rows):\n    return ledger_total(rows)\n",
        ),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .affected_tests(&["ledger_total".into()], 8_000, 0.0, 3)
        .unwrap();
    let tests = &answer.tests.items;
    assert_eq!(tests.len(), 1, "{answer:#?}");
    assert_eq!(tests[0].path, "checks.py");
    assert_eq!(tests[0].depth, 1);
    assert_eq!(tests[0].reached_symbols, 1, "`summary` is not a test");
}

#[test]
fn ordinary_code_outside_a_test_path_is_still_not_a_test() {
    let store = store_of(&[
        (
            "ledger.py",
            "def ledger_total(rows):\n    return sum(rows)\n",
        ),
        (
            "report.py",
            "from ledger import ledger_total\n\n\n\
             def summary(rows):\n    return ledger_total(rows)\n",
        ),
    ]);
    let answer = StoreQueryEngine::new(&store)
        .affected_tests(&["ledger_total".into()], 8_000, 0.0, 3)
        .unwrap();
    assert!(answer.tests.items.is_empty(), "{answer:#?}");
}

/// The store's prefilter matches text, not structure: a file that merely
/// *mentions* `RuntimeEntryPoint` passes it with no `wiring` at all, and must
/// be skipped rather than refused or read as a test.
#[test]
fn a_file_that_only_mentions_an_entry_point_is_not_a_test() {
    let store = store_of(&[
        (
            "ledger.py",
            "def ledger_total(rows):\n    \"\"\"Not a RuntimeEntryPoint, just prose.\"\"\"\n    \
             return sum(rows)\n",
        ),
        (
            "report.py",
            "from ledger import ledger_total\n\n\n\
             def summary(rows):\n    \"\"\"RuntimeEntryPoint test harness invokes it\"\"\"\n    \
             return ledger_total(rows)\n",
        ),
    ]);
    let symbols = store.latest_test_entry_symbols().unwrap();
    assert!(symbols.is_empty(), "{symbols:?}");
    let answer = StoreQueryEngine::new(&store)
        .affected_tests(&["ledger_total".into()], 8_000, 0.0, 3)
        .unwrap();
    assert!(answer.tests.items.is_empty(), "{answer:#?}");
}
