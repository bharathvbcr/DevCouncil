//! A wall-clock budget refusal is the machine's load, not a property of the
//! file's bytes, so it must not outlive the build that suffered it.
//!
//! `file_payloads` is keyed by file + content hash + extractor identity. A
//! later build that parses the same bytes has the same key, and reusing the row
//! unchanged would keep `Failed` as the stored outcome beside nodes the new
//! generation did write. These tests pin the recovered extraction replacing the
//! refused one.

use std::time::Duration;

use devmap_analyze::analyze;
use devmap_extract::{extract_file_with_budget, Extraction};
use devmap_resolve::Resolver;
use devmap_store::Store;

const PATH: &str = "lib.py";
const SRC: &str = "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n";

fn commit(store: &Store, extractions: &[Extraction]) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation(extractions, &resolution, &analysis)
        .unwrap();
}

#[test]
fn a_recovered_extraction_replaces_the_stored_refusal_for_identical_bytes() {
    let store = Store::open_in_memory().unwrap();
    let refused = extract_file_with_budget(PATH, SRC, Duration::ZERO);
    assert!(refused.is_budget_refusal());
    commit(&store, &[refused]);
    assert_eq!(store.latest_budget_refusals().unwrap().unwrap().total, 1);

    let recovered = extract_file_with_budget(PATH, SRC, Duration::from_secs(60));
    assert!(!recovered.is_parse_failure());
    commit(&store, &[recovered]);

    assert_eq!(
        store.latest_budget_refusals().unwrap().unwrap().total,
        0,
        "the refusal outlived the build"
    );
    let stored = store.latest_file(PATH).unwrap().unwrap();
    assert!(
        !matches!(
            stored.parse_outcome,
            devmap_extract::ParseOutcome::Failed { .. }
        ),
        "{:?}",
        stored.parse_outcome
    );
}
