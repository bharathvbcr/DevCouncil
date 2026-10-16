//! Attack on the parse-budget retry's interaction with the extraction cache.
//!
//! `try_get_cached_extraction` falls back to a retained generation's payload
//! (`generation_files`). A generation stores every extraction it was handed,
//! `Failed` ones included, so a budget refusal that survived to
//! `save_generation` can be served back on the next build instead of being
//! re-parsed -- which would make a load-induced refusal permanent.

use std::time::Duration;

use devmap_analyze::analyze;
use devmap_extract::{extract_file_with_budget, ScannedTree};
use devmap_resolve::Resolver;
use devmap_store::{extract_scanned_for_generation, Store};

const PATH: &str = "lib.py";
const SRC: &str = "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n";

fn scanned() -> ScannedTree {
    let mut scanned = ScannedTree::default();
    scanned.sources.push((PATH.into(), SRC.into()));
    scanned
}

fn commit(store: &Store, extractions: &[devmap_extract::Extraction]) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation(extractions, &resolution, &analysis)
        .unwrap();
}

#[test]
fn a_budget_refusal_committed_in_a_generation_is_reparsed_by_the_next_build() {
    let store = Store::open_in_memory().unwrap();
    let refused = extract_file_with_budget(PATH, SRC, Duration::ZERO);
    assert!(refused.is_budget_refusal(), "fixture must be a refusal");
    commit(&store, &[refused]);

    let next = extract_scanned_for_generation(&store, &scanned(), None).unwrap();
    assert_eq!(next.len(), 1);
    assert!(
        !next[0].is_parse_failure(),
        "the next build reused the stored budget refusal instead of re-parsing: {:?}",
        next[0].parse_outcome
    );
}

#[test]
fn a_recovered_extraction_committed_in_a_generation_is_a_cache_hit() {
    let store = Store::open_in_memory().unwrap();
    let recovered = extract_file_with_budget(PATH, SRC, Duration::from_secs(60));
    commit(&store, &[recovered.clone()]);
    let progress = devmap_extract::progress::FileProgress::default();
    let next = extract_scanned_for_generation(&store, &scanned(), Some(&progress)).unwrap();
    assert_eq!(progress.snapshot().cache_hits, 1);
    assert_eq!(next[0].symbols.len(), recovered.symbols.len());
}

#[test]
fn a_still_refused_file_is_named_with_its_reason_in_status() {
    let dir = std::env::temp_dir().join(format!("budget-attack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let refused = extract_file_with_budget(PATH, SRC, Duration::ZERO);
    commit(&store, &[refused]);
    let status = store.status(db.to_str().unwrap()).unwrap();
    let rows = &status.coverage_gaps.parse_failed.shown;
    assert_eq!(rows.len(), 1, "{:?}", status.coverage_gaps);
    assert_eq!(rows[0].path, PATH);
    println!("STATUS REASON: {}", rows[0].reason);
    assert!(rows[0]
        .reason
        .starts_with(devmap_extract::model::BUDGET_REFUSAL_PREFIX));
    let _ = std::fs::remove_dir_all(&dir);
}
