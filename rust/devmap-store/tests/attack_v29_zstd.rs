//! Adversarial tests for schema 29 (zstd extraction documents, `entry_wiring_json`).
//!
//! Written by a reviewer who did not write the lane. Each test names the claim
//! it attacks. A test that FAILS here is a finding; a test that passes is
//! evidence for the claim, not proof of it.
//!
//! Families: codec boundary, wiring consistency, migration/heal, digest
//! invariance, oversize documents.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, WiringKind};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::types::Value;
use rusqlite::{params, Connection};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-attack29-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn commit(store: &Store, extractions: &[Extraction]) -> u32 {
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
        .unwrap()
}

fn small_sources() -> Vec<(String, String)> {
    let mut files = vec![
        (
            "ledger.py".to_string(),
            "LEDGER_NAME = \"ledger\"\n\n\ndef ledger_total(rows):\n    return sum(rows)\n"
                .to_string(),
        ),
        (
            "checks.py".to_string(),
            "from ledger import ledger_total\n\n\ndef test_sums():\n    \
             assert ledger_total([1]) == 1\n\n\ndef summary(rows):\n    return ledger_total(rows)\n"
                .to_string(),
        ),
    ];
    for index in 0..6 {
        files.push((
            format!("pkg/mod_{index}.py"),
            format!("def leaf_{index}():\n    return {index}\n"),
        ));
    }
    files
}

fn extract_all(sources: &[(String, String)]) -> Vec<Extraction> {
    sources.iter().map(|(p, b)| extract_file(p, b)).collect()
}

fn small_store(label: &str) -> (PathBuf, PathBuf) {
    let dir = tmp_dir(label);
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extract_all(&small_sources()));
    drop(store);
    (dir, db)
}

fn one<T: rusqlite::types::FromSql>(db: &Path, sql: &str) -> T {
    Connection::open(db)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

fn decode_cell(value: Value) -> String {
    match value {
        Value::Text(text) => text,
        Value::Blob(frame) => {
            String::from_utf8(zstd::bulk::decompress(&frame, 256 << 20).unwrap()).unwrap()
        }
        other => panic!("unexpected cell {other:?}"),
    }
}

/// Put every payload back to the text v28 wrote and stamp the store 28.
fn reduce_to_v28(db: &Path) {
    let conn = Connection::open(db).unwrap();
    let rows: Vec<(i64, Value)> = conn
        .prepare("SELECT payload_id, extraction_json FROM file_payloads")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (id, cell) in rows {
        conn.execute(
            "UPDATE file_payloads SET extraction_json = ?2 WHERE payload_id = ?1",
            params![id, decode_cell(cell)],
        )
        .unwrap();
    }
    conn.execute_batch(
        "ALTER TABLE file_payloads DROP COLUMN entry_wiring_json;
         UPDATE reader_compat SET min_reader_schema = 28;
         PRAGMA user_version = 28;",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// 1. Codec boundary
// ---------------------------------------------------------------------------

fn set_cell(db: &Path, path: &str, value: Value) {
    let changed = Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE file_payloads SET extraction_json = ?2
              WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
            params![path, value],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

fn cell_of(db: &Path, path: &str) -> Value {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT extraction_json FROM file_payloads
              WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
            params![path],
            |row| row.get(0),
        )
        .unwrap()
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Equal,
    RefusedNamed,
    RefusedUnnamed(String),
    Wrong(String),
}

fn verdict_one(store: &Store, path: &str, baseline: &str) -> Verdict {
    match store.latest_extraction_for_path(path) {
        Ok(Some(extraction)) => {
            if serde_json::to_string(&extraction).unwrap() == baseline {
                Verdict::Equal
            } else {
                Verdict::Wrong(format!(
                    "decoded to a DIFFERENT extraction (file_path={:?}, {} symbols)",
                    extraction.file_path,
                    extraction.symbols.len()
                ))
            }
        }
        Ok(None) => Verdict::Wrong("answered 'no such file'".into()),
        Err(error) => {
            let text = error.to_string();
            if text.contains(path) {
                Verdict::RefusedNamed
            } else {
                Verdict::RefusedUnnamed(text)
            }
        }
    }
}

fn verdict_all(store: &Store, path: &str, baseline: &str) -> Verdict {
    match store.latest_extractions() {
        Ok(all) => {
            let found = all.iter().find(|extraction| extraction.file_path == path);
            match found {
                Some(extraction) if serde_json::to_string(extraction).unwrap() == baseline => {
                    Verdict::Equal
                }
                Some(extraction) => Verdict::Wrong(format!(
                    "latest_extractions returned a different extraction for {path}: {} symbols",
                    extraction.symbols.len()
                )),
                None => Verdict::Wrong(format!("latest_extractions omitted {path} silently")),
            }
        }
        Err(error) => {
            let text = error.to_string();
            if text.contains(path) {
                Verdict::RefusedNamed
            } else {
                Verdict::RefusedUnnamed(text)
            }
        }
    }
}

/// `expect_equal`: the vector is a legitimate encoding of the document, so a
/// correct decode is the right answer; otherwise it must be a loud refusal.
#[test]
fn codec_boundary_every_vector_is_a_correct_decode_or_a_named_refusal() {
    let (dir, db) = small_store("codec");
    let honest_text = decode_cell(cell_of(&db, "checks.py"));
    let honest_frame = match cell_of(&db, "checks.py") {
        Value::Blob(frame) => frame,
        other => panic!("{other:?}"),
    };
    let baseline = {
        let store = Store::open_read_only(&db).unwrap();
        serde_json::to_string(
            &store
                .latest_extraction_for_path("checks.py")
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    };
    let bytes = honest_text.as_bytes();

    let skippable = |payload: &[u8]| {
        let mut frame = vec![0x50, 0x2A, 0x4D, 0x18];
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    };

    // A small single-segment frame of 100 bytes: byte 5 is the 1-byte size.
    let hundred = zstd::bulk::compress(&[b'a'; 100], 1).unwrap();
    assert_eq!((hundred[4], hundred[5]), (0x20, 100));
    let mut declared_bigger = hundred.clone();
    declared_bigger[5] = 200;
    let mut declared_smaller = hundred.clone();
    declared_smaller[5] = 10;

    // A genuine frame with no content size (streamed).
    let streamed = zstd::stream::encode_all(bytes, 1).unwrap();
    let streamed_declares = zstd::zstd_safe::get_frame_content_size(&streamed);

    // A genuine frame without a checksum and with a content size.
    let no_checksum = zstd::bulk::compress(bytes, 1).unwrap();

    let mut honest_then_skippable = honest_frame.clone();
    honest_then_skippable.extend_from_slice(&skippable(b"hello"));
    let mut skippable_then_honest = skippable(b"hello");
    skippable_then_honest.extend_from_slice(&honest_frame);
    let mut honest_then_empty = honest_frame.clone();
    honest_then_empty.extend_from_slice(&zstd::bulk::compress(b"", 1).unwrap());
    let mut honest_then_zeros = honest_frame.clone();
    honest_then_zeros.extend_from_slice(&[0, 0, 0, 0]);
    let mut honest_twice = honest_frame.clone();
    honest_twice.extend_from_slice(&honest_frame);
    let mut magic_then_json = vec![0x28, 0xB5, 0x2F, 0xFD];
    magic_then_json.extend_from_slice(bytes);

    // (label, cell, expect_equal)
    let vectors: Vec<(&str, Value, bool)> = vec![
        ("empty blob", Value::Blob(vec![]), false),
        ("empty text", Value::Text(String::new()), false),
        (
            "text: the honest JSON",
            Value::Text(honest_text.clone()),
            true,
        ),
        (
            "blob: the honest JSON bytes",
            Value::Blob(bytes.to_vec()),
            false,
        ),
        (
            "blob: zstd magic then JSON bytes",
            Value::Blob(magic_then_json),
            false,
        ),
        (
            "skippable frame only (len 0)",
            Value::Blob(skippable(&[])),
            false,
        ),
        (
            "skippable frame only (len 5)",
            Value::Blob(skippable(b"hello")),
            false,
        ),
        (
            "skippable then honest",
            Value::Blob(skippable_then_honest),
            false,
        ),
        // Honest frame first, skippable after: the document decodes to what
        // was stored; accepting it is correct, extra bytes carry no document.
        (
            "honest then skippable",
            Value::Blob(honest_then_skippable),
            true,
        ),
        (
            "honest then empty frame",
            Value::Blob(honest_then_empty),
            true,
        ),
        (
            "honest then 4 zero bytes",
            Value::Blob(honest_then_zeros),
            false,
        ),
        ("honest twice", Value::Blob(honest_twice), false),
        (
            "declared 200, holds 100 (not JSON anyway)",
            Value::Blob(declared_bigger),
            false,
        ),
        (
            "declared 10, holds 100",
            Value::Blob(declared_smaller),
            false,
        ),
        ("frame with no declared size", Value::Blob(streamed), false),
        (
            "valid frame, no checksum, honest document",
            Value::Blob(no_checksum),
            true,
        ),
        (
            "valid frame of an empty document",
            Value::Blob(zstd::bulk::compress(b"", 1).unwrap()),
            false,
        ),
        (
            "valid frame of non-UTF-8 bytes",
            Value::Blob(zstd::bulk::compress(&[0xFF, 0xFE, 0xFD, 0x80], 1).unwrap()),
            false,
        ),
        (
            "valid frame of JSON `{}`",
            Value::Blob(zstd::bulk::compress(b"{}", 1).unwrap()),
            false,
        ),
        (
            "valid frame of JSON `null`",
            Value::Blob(zstd::bulk::compress(b"null", 1).unwrap()),
            false,
        ),
        ("text: `{}`", Value::Text("{}".into()), false),
    ];
    println!("streamed frame content size query: {streamed_declares:?}");

    let mut report = Vec::new();
    for (label, cell, expect_equal) in vectors {
        set_cell(&db, "checks.py", cell);
        let store = Store::open_read_only(&db).unwrap();
        for (api, got) in [
            (
                "latest_extraction_for_path",
                verdict_one(&store, "checks.py", &baseline),
            ),
            (
                "latest_extractions",
                verdict_all(&store, "checks.py", &baseline),
            ),
        ] {
            let ok = matches!(
                (&got, expect_equal),
                (Verdict::Equal, true) | (Verdict::RefusedNamed, false)
            );
            println!("{label:48} {api:28} -> {got:?}");
            if !ok {
                report.push(format!(
                    "{label} / {api}: expected {} got {got:?}",
                    if expect_equal {
                        "Equal"
                    } else {
                        "RefusedNamed"
                    }
                ));
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    assert!(
        report.is_empty(),
        "codec boundary findings:\n{}",
        report.join("\n")
    );
}

/// The same vectors through the *writer's* open path, which converts and heals:
/// a writer open must never turn a bad cell into a silently different one, and
/// must not refuse a store whose only fault is a cell it ignores by design.
#[test]
fn codec_boundary_writer_open_never_rewrites_a_bad_cell_into_a_different_document() {
    let (dir, db) = small_store("codec-writer");
    let honest = cell_of(&db, "checks.py");
    for (label, cell) in [
        ("skippable only", {
            let mut f = vec![0x50, 0x2A, 0x4D, 0x18, 0, 0, 0, 0];
            f.extend_from_slice(&[]);
            Value::Blob(f)
        }),
        ("garbage blob", Value::Blob(vec![1, 2, 3])),
        ("empty text", Value::Text(String::new())),
        ("text not json", Value::Text("{not json".into())),
    ] {
        set_cell(&db, "checks.py", cell.clone());
        // Heal on a same-version writer open: must not fail the open and must
        // leave the cell byte-for-byte as it found it (never "repair" it).
        let opened = Store::open(&db);
        let after = cell_of(&db, "checks.py");
        println!(
            "{label:20} open={} cell_unchanged={}",
            opened.is_ok(),
            after == cell
        );
        assert!(
            opened.is_ok(),
            "{label}: writer open refused a store over one bad cell: {:?}",
            opened.err()
        );
        assert_eq!(
            after, cell,
            "{label}: the heal rewrote a cell it could not read"
        );
        drop(opened);
        set_cell(&db, "checks.py", honest.clone());
    }
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 2. entry_wiring_json consistency, on a real corpus
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn collect(dir: &Path, root: &Path, exts: &[&str], cap: usize, out: &mut Vec<(String, String)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if out.len() >= cap {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "target" || name == "node_modules" || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(&path, root, exts, cap, out);
        } else if exts
            .iter()
            .any(|ext| path.extension().is_some_and(|have| have == *ext))
        {
            if let Ok(body) = fs::read_to_string(&path) {
                if body.len() < 300_000 {
                    out.push((
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .to_string(),
                        body,
                    ));
                }
            }
        }
    }
}

fn corpus() -> Vec<(String, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    collect(
        &root.join("rust/devmap-store"),
        &root,
        &["rs"],
        90,
        &mut files,
    );
    collect(
        &root.join("rust/devmap-resolve/src"),
        &root,
        &["rs"],
        130,
        &mut files,
    );
    collect(&root.join("benchmarks"), &root, &["py"], 150, &mut files);
    collect(
        &root.join("backend/go_orchestrator/cmd"),
        &root,
        &["go"],
        230,
        &mut files,
    );
    collect(
        &root.join("scripts"),
        &root,
        &["py", "js", "mjs", "ts", "sh"],
        270,
        &mut files,
    );
    files
}

/// What `latest_test_entry_symbols` must be, computed from the decoded
/// extractions alone: the v28 semantics.
fn oracle_symbols(db: &Path) -> BTreeSet<String> {
    let store = Store::open_read_only(db).unwrap();
    store
        .latest_extractions()
        .unwrap()
        .iter()
        .flat_map(|extraction| extraction.wiring.iter())
        .filter(|annotation| {
            annotation.kind == WiringKind::RuntimeEntryPoint
                && devmap_extract::wiring::is_test_harness_reason(&annotation.details)
        })
        .map(|annotation| annotation.target_symbol.clone())
        .collect()
}

fn store_symbols(db: &Path) -> BTreeSet<String> {
    Store::open_read_only(db)
        .unwrap()
        .latest_test_entry_symbols()
        .unwrap()
        .into_iter()
        .collect()
}

/// Every payload's stored `entry_wiring_json` against the `RuntimeEntryPoint`
/// annotations of its own decoded document, compared as JSON values so a field
/// the round trip dropped is visible. Returns the mismatching payload paths.
fn wiring_column_mismatches(db: &Path) -> (usize, usize, Vec<String>) {
    let conn = Connection::open(db).unwrap();
    let rows: Vec<(String, Value, Option<String>)> = conn
        .prepare(
            "SELECT p.path, fp.extraction_json, fp.entry_wiring_json
               FROM file_payloads fp JOIN paths p ON p.id = fp.file_id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut bad = Vec::new();
    let mut non_empty = 0;
    for (path, cell, column) in &rows {
        let document: serde_json::Value = serde_json::from_str(&decode_cell(cell.clone())).unwrap();
        let expected: Vec<serde_json::Value> = document["wiring"]
            .as_array()
            .map(|array| {
                array
                    .iter()
                    .filter(|annotation| annotation["kind"] == "RuntimeEntryPoint")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !expected.is_empty() {
            non_empty += 1;
        }
        let Some(column) = column else {
            bad.push(format!("{path}: NULL"));
            continue;
        };
        let stored: serde_json::Value = serde_json::from_str(column).unwrap();
        if stored != serde_json::Value::Array(expected.clone()) {
            bad.push(format!("{path}: stored {column} != document {expected:?}"));
        }
    }
    (rows.len(), non_empty, bad)
}

#[test]
fn wiring_column_equals_the_document_on_a_real_corpus_fresh_migrated_and_healed() {
    let sources = corpus();
    assert!(sources.len() > 150, "corpus too small: {}", sources.len());
    let extractions = extract_all(&sources);
    let dir = tmp_dir("wiring");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    drop(store);

    let oracle = oracle_symbols(&db);
    println!(
        "corpus {} files, oracle test-entry symbols {}",
        sources.len(),
        oracle.len()
    );
    assert!(
        !oracle.is_empty(),
        "the corpus must contain test entry points"
    );
    let (payloads, non_empty, bad) = wiring_column_mismatches(&db);
    println!(
        "fresh: {payloads} payloads, {non_empty} with entry wiring, {} mismatches",
        bad.len()
    );
    assert!(bad.is_empty(), "fresh write: {bad:#?}");
    assert_eq!(
        store_symbols(&db),
        oracle,
        "fresh: latest_test_entry_symbols != oracle"
    );

    // The wiring the migration derives must equal what the writer wrote.
    let fresh_columns: Vec<(String, String)> = {
        let conn = Connection::open(&db).unwrap();
        let rows = conn.prepare("SELECT p.path, fp.entry_wiring_json FROM file_payloads fp JOIN paths p ON p.id = fp.file_id ORDER BY fp.payload_id").unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
            .collect::<Result<_, _>>().unwrap();
        rows
    };
    reduce_to_v28(&db);
    drop(Store::open(&db).unwrap());
    let migrated_columns: Vec<(String, String)> = {
        let conn = Connection::open(&db).unwrap();
        let rows = conn.prepare("SELECT p.path, fp.entry_wiring_json FROM file_payloads fp JOIN paths p ON p.id = fp.file_id ORDER BY fp.payload_id").unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
            .collect::<Result<_, _>>().unwrap();
        rows
    };
    assert_eq!(
        migrated_columns, fresh_columns,
        "the migration and the writer derive different entry wiring (byte-level)"
    );
    assert_eq!(store_symbols(&db), oracle, "migrated");

    // Healed: NULL the column, read-only first (fallback path), then heal.
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE file_payloads SET entry_wiring_json = NULL", [])
        .unwrap();
    assert_eq!(
        store_symbols(&db),
        oracle,
        "NULL column, read through the fallback"
    );
    drop(Store::open(&db).unwrap());
    let healed_columns: Vec<(String, String)> = {
        let conn = Connection::open(&db).unwrap();
        let rows = conn.prepare("SELECT p.path, fp.entry_wiring_json FROM file_payloads fp JOIN paths p ON p.id = fp.file_id ORDER BY fp.payload_id").unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
            .collect::<Result<_, _>>().unwrap();
        rows
    };
    assert_eq!(healed_columns, fresh_columns, "healed");
    assert_eq!(store_symbols(&db), oracle, "healed");
    let _ = fs::remove_dir_all(&dir);
}

/// Wiring that changes between generations: new payloads, reuse of an older
/// payload (A -> B -> A), a deleted test file. `affected_tests` input must track.
#[test]
fn wiring_tracks_edits_reverts_and_deletions_across_generations() {
    let dir = tmp_dir("wiring-edits");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let a = vec![
        (
            "lib.py".to_string(),
            "def helper():\n    return 1\n".to_string(),
        ),
        (
            "t_one.py".to_string(),
            "def test_one():\n    assert 1\n".to_string(),
        ),
    ];
    let b = vec![
        (
            "lib.py".to_string(),
            "def helper():\n    return 1\n".to_string(),
        ),
        (
            "t_one.py".to_string(),
            "def test_one():\n    assert 1\n\n\ndef test_two():\n    assert 2\n".to_string(),
        ),
        (
            "t_new.py".to_string(),
            "def test_three():\n    assert 3\n".to_string(),
        ),
    ];
    commit(&store, &extract_all(&a));
    let (s_a, o_a) = (store_symbols(&db), oracle_symbols(&db));
    commit(&store, &extract_all(&b));
    let (s_b, o_b) = (store_symbols(&db), oracle_symbols(&db));
    commit(&store, &extract_all(&a));
    let (s_a2, o_a2) = (store_symbols(&db), oracle_symbols(&db));
    println!("A={s_a:?}\nB={s_b:?}\nA'={s_a2:?}");
    assert_eq!(s_a, o_a, "generation A");
    assert_eq!(s_b, o_b, "generation B");
    assert_eq!(s_a2, o_a2, "generation A again");
    assert!(s_b.iter().any(|s| s.ends_with("test_two")));
    assert!(s_b.iter().any(|s| s.ends_with("test_three")));
    assert_eq!(
        s_a, s_a2,
        "a revert to A must read as A (reuse of the A payload)"
    );
    let (_, _, bad) = wiring_column_mismatches(&db);
    assert!(bad.is_empty(), "{bad:#?}");
    let _ = fs::remove_dir_all(&dir);
}

/// A reused payload (same digest) whose stored wiring is wrong must not stay
/// wrong forever: this is the write-path invariant "digest equal => column
/// equal". Plant a stale non-NULL column, rebuild the unchanged tree, ask.
#[test]
#[ignore = "card topic: a non-NULL entry_wiring_json is trusted and never re-derived. A characterization, not a defect: no production path writes a stale one (wiring_column_equals_the_document_on_a_real_corpus_fresh_migrated_and_healed holds that). Run with --ignored to see the effect."]
fn a_stale_non_null_wiring_column_is_not_repaired_by_a_rebuild_or_a_heal() {
    let (dir, db) = small_store("stale");
    let before = store_symbols(&db);
    assert!(before.iter().any(|s| s.ends_with("test_sums")));
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE file_payloads SET entry_wiring_json = '[]'", [])
        .unwrap();
    // Heal selects only NULL columns; the rebuild reuses the payload.
    let store = Store::open(&db).unwrap();
    commit(&store, &extract_all(&small_sources()));
    drop(store);
    let after = store_symbols(&db);
    println!("before={before:?} after={after:?}");
    // Documented finding if this is empty: `[]` is trusted, never re-derived.
    assert_eq!(
        after, before,
        "a stale '[]' (non-NULL) wiring column makes affected_tests miss a test and nothing repairs it"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 3. Migration and heal under concurrency
// ---------------------------------------------------------------------------

#[test]
fn racing_writers_and_readers_on_a_v28_store_converge_with_no_bad_state() {
    let (dir, db) = small_store("race");
    let before = store_symbols(&db);
    reduce_to_v28(&db);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut handles = Vec::new();
    for index in 0..8 {
        let db = db.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            if index % 4 == 3 {
                // A reader meets a store that is 28 or 29, nothing else.
                match Store::open_read_only(&db) {
                    Ok(store) => {
                        let _ = store.latest_extractions().expect("reader on v29 answers");
                        "reader: ok".to_string()
                    }
                    Err(error) => format!("reader: refused: {error}"),
                }
            } else {
                match Store::open(&db) {
                    Ok(_) => "writer: ok".to_string(),
                    Err(error) => format!("writer: ERR {error}"),
                }
            }
        }));
    }
    let outcomes: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for outcome in &outcomes {
        println!("{outcome}");
    }
    assert!(
        outcomes.iter().all(|o| !o.starts_with("writer: ERR")),
        "a racing writer failed: {outcomes:?}"
    );
    for outcome in outcomes.iter().filter(|o| o.starts_with("reader: refused")) {
        assert!(
            outcome.contains("schema 28") || outcome.contains("older") || outcome.contains("28"),
            "a reader's refusal must say why: {outcome}"
        );
    }
    assert_eq!(one::<i64>(&db, "PRAGMA user_version"), 29);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) <> 'blob' OR entry_wiring_json IS NULL"
        ),
        0
    );
    assert_eq!(store_symbols(&db), before);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_reader_snapshot_held_across_the_heal_still_reads_one_consistent_world() {
    let (dir, db) = small_store("snapshot");
    let before = store_symbols(&db);
    // Half-convert: every payload back to text, column NULL, stamped 29.
    {
        let conn = Connection::open(&db).unwrap();
        let rows: Vec<(i64, Value)> = conn
            .prepare("SELECT payload_id, extraction_json FROM file_payloads")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (id, cell) in rows {
            conn.execute(
                "UPDATE file_payloads SET extraction_json = ?2, entry_wiring_json = NULL WHERE payload_id = ?1",
                params![id, decode_cell(cell)],
            )
            .unwrap();
        }
    }
    let reader = Store::open_read_only(&db).unwrap();
    let during_a = reader.latest_extractions().unwrap().len();
    // Hold a snapshot with a raw read transaction.
    let raw = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    raw.execute_batch("BEGIN").unwrap();
    let text_rows: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)='text'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(text_rows > 0);
    // The healing writer opens now, concurrently with the held snapshot.
    drop(Store::open(&db).unwrap());
    let still_text: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)='text'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        still_text, text_rows,
        "the held snapshot changed under the reader"
    );
    raw.execute_batch("COMMIT").unwrap();
    let during_b = reader.latest_extractions().unwrap().len();
    assert_eq!(during_a, during_b);
    assert_eq!(store_symbols(&db), before);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_read_only_opener_meets_a_v28_store_with_a_refusal_that_names_the_schema() {
    let (dir, db) = small_store("ro28");
    reduce_to_v28(&db);
    let error = Store::open_read_only(&db)
        .err()
        .expect("a reader cannot read a 28 store")
        .to_string();
    println!("{error}");
    assert!(error.contains("28"), "{error}");
    // And the store was not touched by the refusal.
    assert_eq!(one::<i64>(&db, "PRAGMA user_version"), 28);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)='text'"
        ),
        small_sources().len() as i64
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Kill the migrating process at assorted moments. The store must always be
/// wholly 28 (text, no column) or wholly 29 (frames, column filled).
#[test]
fn killing_the_migration_at_any_moment_leaves_the_store_whole() {
    if let Ok(db) = std::env::var("ATTACK29_MIGRATE_DB") {
        let _ = Store::open(&db);
        return;
    }
    let (dir, db) = small_store("kill");
    reduce_to_v28(&db);
    // Fatten: ~700 KB of incompressible hex per payload so the migration has
    // real work to be killed in the middle of.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "UPDATE file_payloads SET extraction_json =
               substr(extraction_json, 1, length(extraction_json) - 1)
               || ',\"pad\":\"' || hex(randomblob(350000)) || '\"}';
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    }
    let exe = std::env::current_exe().unwrap();
    let mut finished_29 = 0;
    let mut killed_28 = 0;
    for (round, delay_ms) in [2u64, 8, 15, 25, 40, 60, 90, 130, 200, 300, 450, 700]
        .iter()
        .enumerate()
    {
        let trial = dir.join(format!("trial-{round}.sqlite"));
        // Copy the checkpointed v28 file.
        fs::copy(&db, &trial).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args([
                "killing_the_migration_at_any_moment_leaves_the_store_whole",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ATTACK29_MIGRATE_DB", &trial)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(*delay_ms));
        let _ = child.kill();
        let _ = child.wait();
        let conn = Connection::open(&trial).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let text: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)='text'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let blob: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)='blob'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let has_col: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_table_info('file_payloads') WHERE name='entry_wiring_json'", [], |r| r.get(0)).unwrap();
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        println!(
            "delay {delay_ms:4} ms: user_version={version} text={text} blob={blob} has_col={has_col} integrity={integrity}"
        );
        assert_eq!(integrity, "ok");
        match version {
            28 => {
                assert_eq!((blob, has_col), (0, 0), "stamped 28 but partly converted");
                killed_28 += 1;
            }
            29 => {
                assert_eq!(text, 0, "STAMPED 29 WITH TEXT PAYLOADS");
                let null_wiring: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM file_payloads WHERE entry_wiring_json IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(null_wiring, 0);
                finished_29 += 1;
            }
            other => panic!("stamped {other}"),
        }
        drop(conn);
        // Whatever happened, the next writer open completes it.
        drop(Store::open(&trial).expect("the survivor opens"));
        assert_eq!(one::<i64>(&trial, "PRAGMA user_version"), 29);
        assert_eq!(
            one::<i64>(
                &trial,
                "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)<>'blob' OR entry_wiring_json IS NULL"
            ),
            0
        );
    }
    println!("killed-before-commit(28)={killed_28} committed(29)={finished_29}");
    assert!(
        killed_28 > 0 && finished_29 > 0,
        "the kill delays did not straddle the commit (28: {killed_28}, 29: {finished_29}); the test proved nothing"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 4. Digest invariance
// ---------------------------------------------------------------------------

fn digest_rows(db: &Path) -> Vec<(String, Vec<u8>)> {
    let conn = Connection::open(db).unwrap();
    let rows = conn
        .prepare("SELECT p.path, fp.payload_digest FROM file_payloads fp JOIN paths p ON p.id = fp.file_id ORDER BY p.path, fp.payload_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

#[test]
fn digest_is_the_same_fresh_migrated_healed_and_after_a_rebuild_reuses_every_payload() {
    let sources = corpus();
    let extractions = extract_all(&sources[..120]);
    let dir = tmp_dir("digest");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    drop(store);
    let fresh = digest_rows(&db);
    assert!(fresh
        .iter()
        .all(|(_, d)| d.len() == 16 && d.iter().any(|b| *b != 0)));

    // Migrated.
    reduce_to_v28(&db);
    drop(Store::open(&db).unwrap());
    assert_eq!(digest_rows(&db), fresh, "migrated");

    // Healed: a v29 store whose digests were never stamped.
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE file_payloads SET payload_digest = x''", [])
        .unwrap();
    drop(Store::open(&db).unwrap());
    assert_eq!(digest_rows(&db), fresh, "healed from x''");

    // Rebuilding from the same sources reuses every payload (digest equality
    // between the migrated/healed rows and a fresh serialization).
    let payloads = one::<i64>(&db, "SELECT COUNT(*) FROM file_payloads");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    drop(store);
    assert_eq!(
        one::<i64>(&db, "SELECT COUNT(*) FROM file_payloads"),
        payloads,
        "rebuild reused every payload"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 5. Oversize documents
// ---------------------------------------------------------------------------

#[test]
fn a_one_mebibyte_source_file_never_makes_the_document_exceed_what_the_store_will_keep() {
    // The store refuses to WRITE a document over 64 MiB. Discovery admits a
    // source up to 1 MiB. How large can the extraction of such a file be?
    let shapes: Vec<(&str, String, &str)> = vec![
        ("python x=1 lines", "x=1\n".repeat(262_144), "dense.py"),
        (
            "python distinct names",
            (0..70_000).map(|i| format!("v{i}=1\n")).collect(),
            "names.py",
        ),
        (
            "python calls",
            (0..90_000).map(|i| format!("f{i}()\n")).collect(),
            "calls.py",
        ),
        (
            "python one-line defs",
            (0..40_000)
                .map(|i| format!("def f{i}(): return {i}\n"))
                .collect(),
            "defs.py",
        ),
        (
            "rust consts",
            (0..30_000)
                .map(|i| format!("pub const C{i}: u32 = {i};\n"))
                .collect(),
            "consts.rs",
        ),
        (
            "python long distinct strings",
            (0..30_000)
                .map(|i| format!("s{i}='lit-{i}-aaaaaaaaaaaaaaaaaaaa'\n"))
                .collect::<String>(),
            "strs.py",
        ),
        (
            "python attribute chains",
            (0..30_000)
                .map(|i| format!("a{i}=m.n{i}.o{i}.p{i}()\n"))
                .collect::<String>(),
            "chains.py",
        ),
        (
            "rust tiny fns",
            (0..40_000)
                .map(|i| format!("fn f{i}(){{}}\n"))
                .collect::<String>(),
            "fns.rs",
        ),
        (
            "js minified calls",
            (0..60_000)
                .map(|i| format!("a{i}(b{i},c{i});"))
                .collect::<String>(),
            "min.js",
        ),
        (
            "json numbers",
            format!("[{}]", "1,".repeat(500_000) + "1"),
            "numbers.json",
        ),
    ];
    let mut worst = 0usize;
    for (label, body, path) in shapes {
        let body = &body[..body.len().min(1 << 20)];
        let extraction = extract_file(path, body);
        let mut durable = extraction.for_durable_store();
        durable.source_code = None;
        let json = serde_json::to_string(&durable).unwrap();
        println!(
            "{label:24} source {:>8} B -> extraction JSON {:>10} B ({:.1}x)",
            body.len(),
            json.len(),
            json.len() as f64 / body.len() as f64
        );
        worst = worst.max(json.len());
    }
    println!(
        "worst extraction JSON: {worst} bytes (limit {})",
        64 * 1024 * 1024
    );
    assert!(
        worst < 64 * 1024 * 1024,
        "a legal 1 MiB source can produce a document the store refuses to write"
    );
}

// ---------------------------------------------------------------------------
// 6. Fail-before probes: claims that can be stated without the v29 API
// ---------------------------------------------------------------------------

/// "The extraction column is stored compressed": stated through raw SQL only,
/// so it compiles and runs against the schema-28 tree, where it must fail.
#[test]
fn the_extraction_column_is_much_smaller_than_the_documents_it_holds() {
    let sources = corpus();
    let extractions = extract_all(&sources[..150]);
    let dir = tmp_dir("size");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    drop(store);
    let stored: i64 = one(
        &db,
        "SELECT SUM(length(CAST(extraction_json AS BLOB))) FROM file_payloads",
    );
    let raw: usize = extractions
        .iter()
        .map(|extraction| {
            let mut durable = extraction.for_durable_store();
            durable.source_code = None;
            serde_json::to_string(&durable).unwrap().len()
        })
        .sum();
    println!(
        "stored {stored} B, raw JSON {raw} B, ratio {:.2}",
        raw as f64 / stored as f64
    );
    assert!(
        (stored as usize) * 4 < raw,
        "stored {stored} B is not under a quarter of the {raw} B of JSON"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Parity claim, stateable on both trees: the test-entry answer equals the
/// decoded-extraction oracle. Passes on the base too (non-discriminating by
/// construction: it is a "nothing changed" claim).
#[test]
fn the_test_entry_answer_equals_the_decoded_extraction_oracle() {
    let sources = corpus();
    let extractions = extract_all(&sources[..150]);
    let dir = tmp_dir("parity");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    drop(store);
    let oracle = oracle_symbols(&db);
    assert!(!oracle.is_empty());
    assert_eq!(store_symbols(&db), oracle);
    let _ = fs::remove_dir_all(&dir);
}

/// Real processes, not threads: `Store::open` serialises in-process openers on
/// a mutex, so only separate processes race the SQLite write lock.
#[test]
fn separate_processes_racing_the_migration_all_succeed() {
    if let Ok(db) = std::env::var("ATTACK29_RACE_DB") {
        Store::open(&db).expect("racing writer");
        return;
    }
    let (dir, db) = small_store("procrace");
    let before = store_symbols(&db);
    reduce_to_v28(&db);
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "UPDATE file_payloads SET extraction_json =
               substr(extraction_json, 1, length(extraction_json) - 1)
               || ',\"pad\":\"' || hex(randomblob(200000)) || '\"}';
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    }
    let exe = std::env::current_exe().unwrap();
    let mut children: Vec<_> = (0..6)
        .map(|_| {
            std::process::Command::new(&exe)
                .args([
                    "separate_processes_racing_the_migration_all_succeed",
                    "--exact",
                    "--test-threads=1",
                ])
                .env("ATTACK29_RACE_DB", &db)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut failures = Vec::new();
    for child in &mut children {
        let output = child.wait_with_output_compat();
        if !output.0 {
            failures.push(output.1);
        }
    }
    assert!(
        failures.is_empty(),
        "racing processes failed: {failures:#?}"
    );
    assert_eq!(one::<i64>(&db, "PRAGMA user_version"), 29);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json)<>'blob' OR entry_wiring_json IS NULL"
        ),
        0
    );
    assert_eq!(store_symbols(&db), before);
    let _ = fs::remove_dir_all(&dir);
}

trait WaitCompat {
    fn wait_with_output_compat(&mut self) -> (bool, String);
}
impl WaitCompat for std::process::Child {
    fn wait_with_output_compat(&mut self) -> (bool, String) {
        use std::io::Read;
        let status = self.wait().unwrap();
        let mut stderr = String::new();
        if let Some(pipe) = self.stderr.as_mut() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        (status.success(), stderr.chars().take(600).collect())
    }
}

// ---------------------------------------------------------------------------
// 7. The fix-up pass that re-reads and rewrites payloads (scrub), and open cost
// ---------------------------------------------------------------------------

/// A pre-rule store holds an unwithheld literal in a secret file's payload.
/// The write transaction's scrub must rewrite the compressed document, keep it
/// a frame, drop the value, and leave `entry_wiring_json` equal to the rewritten
/// document's.
#[test]
fn the_secret_scrub_rewrites_a_frame_and_keeps_the_entry_wiring_consistent() {
    let candidates = [
        "credentials/keys.py",
        "secrets/keys.py",
        "config/secrets/keys.py",
        "keys/credentials.py",
        "private/secrets.py",
    ];
    let secret_path = candidates
        .iter()
        .find(|path| devmap_extract::secret::is_secret_path(path))
        .expect("no candidate path is a secret path; adjust the candidates");
    let dir = tmp_dir("scrub");
    let db = dir.join("devmap.sqlite");
    let mut sources = small_sources();
    sources.push((
        secret_path.to_string(),
        "TOKEN = \"hunter2-not-a-real-secret\"\n\n\ndef test_secret_entry():\n    assert TOKEN\n"
            .to_string(),
    ));
    let extractions = extract_all(&sources);
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions);
    // Plant the pre-rule shape: the payload text holds the value, stored as
    // TEXT as a v28 writer left it, with the wiring column cleared.
    {
        let conn = Connection::open(&db).unwrap();
        let cell: Value = conn
            .query_row(
                "SELECT extraction_json FROM file_payloads
                  WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
                params![secret_path],
                |row| row.get(0),
            )
            .unwrap();
        let text = decode_cell(cell);
        assert!(
            text.contains("<withheld: secret_path>"),
            "fixture: the extractor withholds at the source"
        );
        let planted = text.replace("<withheld: secret_path>", "hunter2-not-a-real-secret");
        conn.execute(
            "UPDATE file_payloads SET extraction_json = ?2, entry_wiring_json = '[]'
              WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
            params![secret_path, planted],
        )
        .unwrap();
    }
    // Any later build runs the scrub inside its write transaction.
    commit(&store, &extractions);
    drop(store);
    let (kind, document, column): (String, Value, String) = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT typeof(extraction_json), extraction_json, entry_wiring_json FROM file_payloads
              WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
            params![secret_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let text = decode_cell(document);
    println!("after scrub: typeof={kind} wiring_column={column}");
    assert!(
        !text.contains("hunter2"),
        "the scrub left the value in the stored document"
    );
    assert_eq!(
        kind, "blob",
        "the scrub left a TEXT document in a v29 store"
    );
    let (_, _, bad) = wiring_column_mismatches(&db);
    assert!(
        bad.is_empty(),
        "scrub left the column out of step: {bad:#?}"
    );
    assert_eq!(store_symbols(&db), oracle_symbols(&db));
    let _ = fs::remove_dir_all(&dir);
}

/// Open cost of a writer on a real store, min of 7. Skipped unless
/// ATTACK29_REAL_DB names a v29 store (a COPY: a writer open mutates).
#[test]
#[ignore = "card topic: store cost measurements (v29 writer-open and status cost on a real store). A measurement, not a pass/fail gate: it needs ATTACK29_REAL_DB and a quiet machine, and an unset variable returns early reporting a pass for a check that did not run"]
fn a_writer_open_of_a_converged_real_store_costs_little() {
    let Ok(source) = std::env::var("ATTACK29_REAL_DB") else {
        println!("skipped: ATTACK29_REAL_DB not set");
        return;
    };
    let dir = tmp_dir("realopen");
    let db = dir.join("devmap.sqlite");
    fs::copy(&source, &db).unwrap();
    drop(Store::open(&db).unwrap());
    let mut best = u128::MAX;
    for _ in 0..7 {
        let started = std::time::Instant::now();
        drop(Store::open(&db).unwrap());
        best = best.min(started.elapsed().as_millis());
    }
    println!("writer open of a converged store: min of 7 = {best} ms");
    assert!(best < 1000, "{best} ms");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_migrated_store_refuses_a_v28_reader_by_its_floor() {
    let (dir, db) = small_store("floor");
    reduce_to_v28(&db);
    assert_eq!(
        one::<i64>(&db, "SELECT min_reader_schema FROM reader_compat"),
        28
    );
    drop(Store::open(&db).unwrap());
    assert_eq!(
        one::<i64>(&db, "SELECT min_reader_schema FROM reader_compat"),
        29,
        "the migration left the floor where a v28 reader would be admitted"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Round 2: status honesty (`degraded_reason` for undecodable payloads).
// ---------------------------------------------------------------------------

fn encoding_reason(store: &Store, db: &Path) -> Option<String> {
    store
        .status(&db.to_string_lossy())
        .unwrap()
        .degraded_reason
        .filter(|reason| reason.contains("schema-29 form"))
}

fn insert_text_payload(conn: &Connection, id: i64, path: &str, document: &str) {
    conn.execute(
        "INSERT INTO paths (id, path) VALUES (?1, ?2)",
        params![id, path],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_payloads
           (file_id, content_hash, language, grammar_version, analyzer_version,
            parse_outcome_json, engine_json, extraction_json)
         VALUES (?1, ?1, 'python', 'g', 'a', '\"Clean\"', '\"TreeSitter\"', ?2)",
        params![id, document],
    )
    .unwrap();
}

/// FINDING (FIX): a corrupt zstd frame whose `entry_wiring_json` is already
/// set is outside the question the status asks (`typeof <> 'blob' OR wiring IS
/// NULL`), so `degraded_reason` stays null while a reader refuses the row. Bit
/// rot and a truncated write both look like this; neither is a writer-open
/// failure, but both are "a payload that cannot be decoded".
#[test]
#[ignore = "card topic: status scope for an undecodable frame that already has its entry wiring. Fails by design on 009f2d7a: a frame-header check was measured (status 1.9 ms -> 5.8 ms) and not taken, and the degraded_reason states its scope instead; v29_compressed_extractions pins the gap and the wording"]
fn status_reports_a_corrupt_frame_that_still_has_its_wiring() {
    let (dir, db) = small_store("status-corrupt-frame");
    set_cell(
        &db,
        "checks.py",
        Value::Blob(vec![0x28, 0xb5, 0x2f, 0xfd, 0, 1, 2]),
    );
    let store = Store::open_read_only(&db).unwrap();
    assert!(
        store.latest_extraction_for_path("checks.py").is_err(),
        "the reader refuses the row"
    );
    let reason = store
        .status(&db.to_string_lossy())
        .unwrap()
        .degraded_reason
        .unwrap_or_default();
    assert!(reason.contains("checks.py"), "status names it: {reason:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// The 65th unconverted row is bad; the first 64 are readable text. The reason
/// must be non-null and say it did not examine the rest.
#[test]
fn a_bad_row_past_the_examination_cap_is_not_read_as_clean() {
    let (dir, db) = small_store("status-past-cap");
    {
        let conn = Connection::open(&db).unwrap();
        for index in 0..64 {
            insert_text_payload(&conn, 2000 + index, &format!("ok/f{index}.py"), "{}");
        }
        insert_text_payload(&conn, 3000, "bad/last.py", "{not json");
    }
    let store = Store::open_read_only(&db).unwrap();
    let reason = encoding_reason(&store, &db).expect("65 unconverted rows cannot be certified");
    assert!(reason.contains("not examined"), "{reason}");
    assert!(
        reason.contains("1 more"),
        "counts exactly the one unexamined row: {reason}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// More than 64 bad rows: the count of failures is of those examined and the
/// remainder is named as unexamined; neither number may be presented as total.
#[test]
fn many_bad_rows_report_both_the_examined_failures_and_the_unexamined_rest() {
    let (dir, db) = small_store("status-many-bad");
    {
        let conn = Connection::open(&db).unwrap();
        for index in 0..70 {
            insert_text_payload(
                &conn,
                2000 + index,
                &format!("bad/f{index}.py"),
                "{not json",
            );
        }
    }
    let store = Store::open_read_only(&db).unwrap();
    let reason = encoding_reason(&store, &db).unwrap();
    assert!(
        reason.contains("64 stored extraction(s) cannot be converted"),
        "{reason}"
    );
    assert!(reason.contains("and 59 more"), "{reason}");
    assert!(reason.contains("6 more extraction(s)"), "{reason}");
    let _ = fs::remove_dir_all(&dir);
}

/// A bad row created after the store was opened, by another connection, is seen
/// by the same `Store` instance's next status (nothing is memoised).
#[test]
fn a_bad_row_created_after_open_is_seen_by_the_open_store() {
    let (dir, db) = small_store("status-after-open");
    let store = Store::open(&db).unwrap();
    assert_eq!(encoding_reason(&store, &db), None);
    insert_text_payload(
        &Connection::open(&db).unwrap(),
        4000,
        "late/bad.py",
        "{not json",
    );
    let reason = encoding_reason(&store, &db).expect("seen without reopening");
    assert!(reason.contains("late/bad.py"), "{reason}");
    drop(store);
    let _ = fs::remove_dir_all(&dir);
}

/// NIT: at the store layer the path is verbatim, so a newline or a bidi
/// override reaches `StoreStatus::degraded_reason` raw. The daemon and the CLI
/// both route it through `store_degraded_reason`, which applies `bounded_reason`
/// (escapes `char::is_control`, i.e. Cc only). A bidi override (Cf) is not
/// escaped there, the same as the quarantined paths beside it.
#[test]
fn a_path_with_controls_is_raw_at_the_store_layer() {
    let (dir, db) = small_store("status-ctl-path");
    insert_text_payload(
        &Connection::open(&db).unwrap(),
        5000,
        "evil\nline\u{1b}[2J\u{202e}gnp.py",
        "{not json",
    );
    let store = Store::open_read_only(&db).unwrap();
    let reason = encoding_reason(&store, &db).unwrap();
    assert!(reason.contains('\n') && reason.contains('\u{1b}') && reason.contains('\u{202e}'));
    let _ = fs::remove_dir_all(&dir);
}

/// Cost: a healthy converted store pays one full scan of `file_payloads` for
/// the encoding question, twice per status (the snapshot is taken twice when a
/// generation exists). Prints min-of-9 of status and of the bare question.
#[test]
#[ignore = "card topic: store cost measurements (v29 status cost of the encoding question). A measurement that prints min-of-9 timings and asserts none of them; it is no evidence under load"]
fn status_cost_of_the_encoding_question_on_a_real_sized_store() {
    let dir = tmp_dir("status-cost");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let sources = corpus();
    commit(&store, &extract_all(&sources));
    let rows: i64 = one(&db, "SELECT COUNT(*) FROM file_payloads");
    let bytes: i64 = one(
        &db,
        "SELECT SUM(LENGTH(extraction_json)) FROM file_payloads",
    );
    let name = db.to_string_lossy().to_string();
    let min_of = |f: &dyn Fn()| {
        (0..9)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed().as_micros()
            })
            .min()
            .unwrap()
    };
    let conn = Connection::open(&db).unwrap();
    let question = || {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_payloads
                  WHERE typeof(extraction_json) <> 'blob' OR entry_wiring_json IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    };
    let status = min_of(&|| {
        store.status(&name).unwrap();
    });
    let bare = min_of(&question);
    println!("rows={rows} stored_bytes={bytes} status_min_us={status} question_min_us={bare}");
    assert_eq!(encoding_reason(&store, &db), None);
    drop(store);
    let _ = fs::remove_dir_all(&dir);
}
