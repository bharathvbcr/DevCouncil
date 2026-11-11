//! A generation carries the identity of the resolver that wrote its edges.
//!
//! `latest_generation_payload_is_current` used to compare only the extractor
//! identity, so a resolver-only change (a `RESOLUTION_REUSE_EPOCH` move) left
//! every stored edge reported as current (ft-c1d8cb8e). The store's own write
//! path now stamps `$.resolver_epoch` into `generations.analysis_json` (a
//! free-form document: no table, column or schema version moves), and the
//! currency check reads it back. An absent stamp is not a matching one.
//! `latest_generation_matches_working_tree` stays extractor-only, so seeding and
//! stacking judge a donor as they did before the stamp.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-resolver-epoch-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// A store with one generation, written by this binary's own write path.
fn indexed(label: &str) -> (PathBuf, PathBuf, Store) {
    let root = tmp_dir(label);
    fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    let (files, _report) = extract_tree_with_report(&root).unwrap();
    assert!(
        !files.is_empty(),
        "fixture precondition: the file was indexed"
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
    let db = root.join("store.sqlite");
    let store = Store::open(&db).unwrap();
    store
        .save_generation_with_metadata(
            &files,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
            "unavailable",
        )
        .unwrap();
    (root, db, store)
}

fn stamp(db: &Path) -> Option<String> {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT json_extract(analysis_json, '$.resolver_epoch')
               FROM generations WHERE id = (SELECT max(id) FROM generations)",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn exec(db: &Path, sql: &str) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let changed = conn
        .execute(
            &format!(
                "UPDATE generations SET analysis_json = {sql}
                  WHERE id = (SELECT max(id) FROM generations)"
            ),
            [],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

#[test]
fn the_write_path_stamps_the_resolver_epoch_and_the_store_is_current() {
    let (root, db, store) = indexed("stamp");
    assert_eq!(
        stamp(&db).as_deref(),
        Some(RESOLUTION_REUSE_EPOCH),
        "the generation is stamped with the epoch of the resolver that wrote it"
    );
    assert!(store.latest_generation_payload_is_current().unwrap());
    assert!(store.latest_generation_matches_working_tree().unwrap());
    let status = store.status("test").unwrap();
    assert_eq!(status.analyzer_freshness, Some(true));
    assert!(status.is_fresh(), "{:?}", status.degraded_reason);
    // The summary still reads back: the extra key is invisible to its readers.
    assert!(store.latest_analysis().unwrap().is_some());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_older_resolver_stamp_is_not_current_and_status_says_why() {
    let (root, db, store) = indexed("older");
    exec(&db, "json_set(analysis_json, '$.resolver_epoch', '1')");
    assert!(
        !store.latest_generation_payload_is_current().unwrap(),
        "edges resolved under epoch 1 are not current for epoch {RESOLUTION_REUSE_EPOCH}"
    );
    assert!(
        store.latest_generation_matches_working_tree().unwrap(),
        "seeding and stacking judge a donor by the extractor alone"
    );
    let status = store.status("test").unwrap();
    assert_eq!(status.analyzer_freshness, Some(false));
    assert!(!status.is_fresh());
    let reason = status.degraded_reason.expect("a reason is named");
    assert!(
        reason.contains("older resolver epoch") && reason.contains("devmap build"),
        "{reason}"
    );
    assert!(
        !reason.contains("obsolete"),
        "an extractor-payload reason is what makes SessionStart rebuild; this one must not: {reason}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_absent_stamp_is_not_current_and_reads_as_unrecorded() {
    let (root, db, store) = indexed("absent");
    exec(&db, "json_remove(analysis_json, '$.resolver_epoch')");
    assert_eq!(stamp(&db), None);
    assert!(!store.latest_generation_payload_is_current().unwrap());
    assert!(store.latest_generation_matches_working_tree().unwrap());
    let reason = store
        .status("test")
        .unwrap()
        .degraded_reason
        .expect("a reason is named");
    assert!(reason.contains("unrecorded"), "{reason}");
    let _ = fs::remove_dir_all(&root);
}

/// A check that could not read the stamp must not report what a check that
/// passed reports: an unreadable summary is stale, not current.
#[test]
fn an_unreadable_analysis_document_is_not_current() {
    let (root, db, store) = indexed("malformed");
    exec(&db, "'{not json'");
    assert!(!store.latest_generation_payload_is_current().unwrap());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_store_with_no_generation_is_current_as_before() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.latest_generation_payload_is_current().unwrap());
}
