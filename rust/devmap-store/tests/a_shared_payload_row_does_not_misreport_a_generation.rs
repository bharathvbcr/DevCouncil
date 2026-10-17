//! Attack on the in-place rewrite of a shared `file_payloads` row.
//!
//! `file_payloads` is shared by every generation that names the same
//! file + content + extractor identity, and `generation_files` is a *view*
//! over it (no separate copy of `parse_outcome_json`). So a payload that is
//! rewritten for the newest generation is rewritten for every retained older
//! one, and a payload that is *not* rewritten when a refusal arrives over a
//! good row makes the newer generation read a parse its graph never held.

use std::time::Duration;

use devmap_analyze::analyze;
use devmap_extract::{extract_file_with_budget, Extraction, ParseOutcome};
use devmap_resolve::Resolver;
use devmap_store::Store;
use rusqlite::{params, Connection};

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

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `(stored outcome, symbols the extraction payload holds, nodes the
/// generation's own graph holds)` for `PATH` in `generation`.
fn as_stored(db: &std::path::Path, generation: i64) -> (ParseOutcome, usize, i64) {
    let conn = Connection::open(db).unwrap();
    let (outcome, extraction): (String, String) = conn
        .query_row(
            "SELECT f.parse_outcome_json, f.extraction_json FROM generation_files f
             JOIN paths p ON p.id = f.file_id
             WHERE f.generation_id = ?1 AND p.path = ?2",
            params![generation, PATH],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let nodes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_nodes n JOIN paths p ON p.id = n.file_id
             WHERE n.generation_id = ?1 AND p.path = ?2",
            params![generation, PATH],
            |row| row.get(0),
        )
        .unwrap();
    let extraction: Extraction = serde_json::from_str(&extraction).unwrap();
    (
        serde_json::from_str(&outcome).unwrap(),
        extraction.symbols.len(),
        nodes,
    )
}

#[test]
#[ignore = "known failing until ft-3bdf1450 (payload identity distinguishes a budget refusal)"]
fn a_retained_older_generation_keeps_the_refusal_it_was_built_with() {
    let dir = scratch("shared-payload-forward");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let refused = extract_file_with_budget(PATH, SRC, Duration::ZERO);
    assert!(refused.is_budget_refusal());
    commit(&store, &[refused]);
    let (outcome, symbols, nodes) = as_stored(&db, 1);
    assert!(matches!(outcome, ParseOutcome::Failed { .. }));
    let before = (symbols, nodes);
    assert_eq!(
        symbols as i64, nodes,
        "a generation's payload and graph agree"
    );

    commit(
        &store,
        &[extract_file_with_budget(PATH, SRC, Duration::from_secs(60))],
    );
    let (outcome, symbols, nodes) = as_stored(&db, 1);
    // Generation 1's graph is what it was. Its stored outcome and extraction
    // must not change to a parse that its graph never contained.
    assert!(
        matches!(outcome, ParseOutcome::Failed { .. })
            && (symbols, nodes) == before
            && symbols as i64 == nodes,
        "retained generation 1 now reads outcome={outcome:?} with {symbols} extracted \
         symbols while its own graph holds {nodes} nodes for the file (was {before:?})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refusal_over_a_stored_good_payload_is_still_named() {
    let dir = scratch("shared-payload-reverse");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    commit(
        &store,
        &[extract_file_with_budget(PATH, SRC, Duration::from_secs(60))],
    );
    let refused = extract_file_with_budget(PATH, SRC, Duration::ZERO);
    assert!(refused.is_budget_refusal());
    commit(&store, &[refused]);

    let (outcome, symbols, nodes) = as_stored(&db, 2);
    let named = store.latest_budget_refusals().unwrap().unwrap().total;
    // Generation 2 was built from a refusal, so its graph holds only what the
    // refused extraction carried. Either the payload agrees with the graph, or
    // the stored outcome / the refusal listing says the file was refused.
    assert!(
        symbols as i64 == nodes || named == 1 || matches!(outcome, ParseOutcome::Failed { .. }),
        "generation 2 holds {nodes} nodes for {PATH} yet reads outcome={outcome:?} with \
         {symbols} extracted symbols, and latest_budget_refusals names {named}: the \
         refused file is invisible to status"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
