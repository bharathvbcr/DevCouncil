//! A build without the parsing frontend must still be able to call a store current.
//!
//! GitPulse links `devmap-query` with `default-features = false`: it answers
//! questions about a persisted map and never builds one. Whether that map is
//! current depends on the grammar identity a parsing build stamps on every
//! payload, and that identity used to be computable only with grammars linked.
//! So every `devmap_status` GitPulse answered said `analyzer_freshness: null`,
//! "this binary was built without the parsing frontend", for a store the CLI
//! reported fresh.
//!
//! This target lives here, not in `devmap-store`, on purpose. `devmap-store`'s
//! test build pulls `devmap-serve` in as a dev-dependency, which turns `parse`
//! back on, so `cargo test -p devmap-store --no-default-features` never runs a
//! store test without grammars. `devmap-query`'s test build does stay
//! grammar-free, and this crate is the one GitPulse links.
//!
//! The rows are written by SQL because a build without grammars writes no
//! generation. They are stamped with the identities a parsing build of this
//! version stamps: `devmap_extract::cache` pins its committed table to the
//! compiled grammars in `the_committed_grammar_identities_are_the_compiled_ones`,
//! and this test asserts the other half, that a reader without them uses it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::Store;
use rusqlite::{params, Connection};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-query-only-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A one-generation store whose `python` payload carries `python_grammar`, and
/// whose `markdown` payload carries what a parsing build stamps for a language
/// it has no grammar for.
fn store_stamped(dir: &Path, python_grammar: &str) -> PathBuf {
    let db = dir.join("devmap.sqlite");
    drop(Store::open(&db).expect("a fresh store must open"));
    let (_, analyzer) = devmap_extract::cache::current_payload_identity("python");
    let (markdown_grammar, _) = devmap_extract::cache::current_payload_identity("markdown");
    assert_eq!(
        markdown_grammar, "unavailable:markdown",
        "fixture precondition: markdown has no grammar in any build"
    );
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO paths (id, path) VALUES (1, 'a.py'), (2, 'README.md');
        INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
        VALUES (1, 1.0, 'deadbeef', NULL, '{"total_files":2,"total_symbols":0,
                "total_edges":0,"dead_symbols":[],"communities":[],"status":"Ok"}');
        "#,
    )
    .unwrap();
    let mut payload = conn
        .prepare(
            "INSERT INTO file_payloads
                 (payload_id, file_id, content_hash, language, grammar_version,
                  analyzer_version, parse_outcome_json, engine_json, extraction_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '\"Clean\"', '\"TreeSitter\"', ?7)",
        )
        .unwrap();
    payload
        .execute(params![
            1,
            1,
            99,
            "python",
            python_grammar,
            analyzer,
            r#"{"file_path":"a.py"}"#
        ])
        .unwrap();
    payload
        .execute(params![
            2,
            2,
            77,
            "markdown",
            markdown_grammar,
            analyzer,
            r#"{"file_path":"README.md"}"#
        ])
        .unwrap();
    conn.execute_batch(
        "INSERT INTO generation_file_rows (generation_id, file_id, payload_id)
         VALUES (1, 1, 1), (1, 2, 2);",
    )
    .unwrap();
    db
}

#[test]
fn a_store_stamped_by_this_version_is_current_to_a_reader_without_grammars() {
    let dir = tmp_dir("current");
    let (python_grammar, _) = devmap_extract::cache::current_payload_identity("python");
    assert!(
        python_grammar.starts_with("tree-sitter-python@"),
        "python's identity must name its grammar in every build, got {python_grammar}"
    );
    let db = store_stamped(&dir, &python_grammar);
    let reader = Store::open_read_only(&db).unwrap();

    assert!(
        reader.latest_generation_payload_is_current().unwrap(),
        "a store stamped with this version's identities must be current"
    );
    let status = reader.status(&db.to_string_lossy()).unwrap();
    assert_eq!(
        status.analyzer_freshness,
        Some(true),
        "status must establish analyzer freshness without grammars: {:?}",
        status.degraded_reason
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_store_stamped_by_another_grammar_is_stale_to_a_reader_without_grammars() {
    let dir = tmp_dir("stale");
    let db = store_stamped(&dir, "tree-sitter-python@0.0.1:python:abi14");
    let reader = Store::open_read_only(&db).unwrap();

    assert!(
        !reader.latest_generation_payload_is_current().unwrap(),
        "a payload stamped by a different grammar is not current"
    );
    let status = reader.status(&db.to_string_lossy()).unwrap();
    assert_eq!(status.analyzer_freshness, Some(false));
    let _ = fs::remove_dir_all(&dir);
}
