//! Behavioural claims of schema v29 that need none of its API: raw SQL and the
//! public `Store` surface only, so this file compiles against the schema-28
//! tree and *fails there for the reason the bump exists*, not for a missing
//! setup step (`v29_compressed_extractions.rs` cannot say that: it needs the
//! `zstd` crate and `entry_wiring_json`, so on the base it fails first on a
//! precondition).
//!
//! * the stored extraction documents are a fraction of the JSON they hold, so a
//!   payload that went back to TEXT fails here as well as in the size gates;
//! * `latest_test_entry_symbols`, the query on every hook-driven
//!   `affected_tests`, is served without reading the extraction document.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::Connection;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "devmap-v29-size-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn commit(store: &Store, extractions: &[Extraction]) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation_with_opts(
            extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
}

/// This crate's own sources: real Rust, in the quantity the ratio needs, and
/// stable for as long as the crate has a `src` directory.
fn own_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let body = fs::read_to_string(&path).unwrap();
                if body.len() < 300_000 {
                    out.push((
                        path.strip_prefix(root).unwrap().to_string_lossy().into(),
                        body,
                    ));
                }
            }
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(&root.join("src"), &root, &mut files);
    files
}

/// **The extraction documents are compressed.** Stated from outside: what the
/// store holds for a payload's extraction is compared with the JSON the writer
/// is contracted to store (`for_durable_store`, no source text), serialised
/// here, so the claim needs no decoder. Measured 11x on this repository's
/// whole store; this crate's own sources are the smaller sample, and the bound
/// is 6x. Text storage is 1x and fails, and so does a store that compresses
/// something other than the documents.
#[test]
fn the_stored_extraction_documents_are_a_fraction_of_the_json_they_hold() {
    let sources = own_sources();
    assert!(
        sources.len() >= 20,
        "the sample must be a real corpus: {}",
        sources.len()
    );
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let json_bytes: usize = extractions
        .iter()
        .map(|extraction| {
            let mut durable = extraction.for_durable_store();
            durable.source_code = None;
            serde_json::to_string(&durable).unwrap().len()
        })
        .sum();

    let dir = tmp_dir("ratio");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    drop(store);

    let (payloads, stored): (i64, i64) = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*), SUM(LENGTH(extraction_json)) FROM file_payloads",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(payloads as usize, extractions.len());
    assert!(
        (stored as usize) * 6 < json_bytes,
        "{stored} bytes are stored for {json_bytes} bytes of extraction JSON \
         ({:.1}x); schema 29 stores each document as a zstd frame, about 11x",
        json_bytes as f64 / stored as f64
    );
    let _ = fs::remove_dir_all(&dir);
}

/// **The hook-driven `affected_tests` query does not read the documents.**
/// Every stored extraction is replaced by text that mentions `RuntimeEntryPoint`
/// and is not JSON: the v28 query (`instr(extraction_json, 'RuntimeEntryPoint')`
/// then `json_extract`) meets it on every row and fails; the v29 query is served
/// by `entry_wiring_json` and never opens a document, so its answer is the one
/// it gave before the documents were damaged. Opened read-only, so no writer
/// heals the damage first.
#[test]
fn the_test_entry_query_is_served_without_reading_the_extraction_documents() {
    let files = [
        (
            "ledger.py",
            "def ledger_total(rows):\n    return sum(rows)\n",
        ),
        (
            "checks.py",
            "from ledger import ledger_total\n\n\ndef test_sums():\n    \
             assert ledger_total([1]) == 1\n",
        ),
    ];
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let dir = tmp_dir("hot-path");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    let expected = store.latest_test_entry_symbols().unwrap();
    drop(store);
    assert!(
        expected.iter().any(|symbol| symbol.ends_with("test_sums")),
        "the fixture must have a test entry point: {expected:?}"
    );

    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE file_payloads SET extraction_json = 'RuntimeEntryPoint {not json'",
            [],
        )
        .unwrap();
    let store = Store::open_read_only(&db).unwrap();
    assert_eq!(
        store
            .latest_test_entry_symbols()
            .expect("the query must not need the documents"),
        expected
    );
    let _ = fs::remove_dir_all(&dir);
}
