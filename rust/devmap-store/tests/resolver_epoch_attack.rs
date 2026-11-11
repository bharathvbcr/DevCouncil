//! Adversarial attacks on the resolver-epoch stamp (ft-c1d8cb8e). Each test
//! asserts the CORRECT behaviour; a failing test is a finding. Not run when
//! written: the host was under the disk floor (see the findings file).

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
        "devmap-epoch-attack-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn write_generation(root: &Path, store: &Store) {
    let (files, _report) = extract_tree_with_report(root).unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
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
}

fn indexed(label: &str) -> (PathBuf, PathBuf, Store) {
    let root = tmp_dir(label);
    fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    let db = root.join("store.sqlite");
    let store = Store::open(&db).unwrap();
    write_generation(&root, &store);
    (root, db, store)
}

fn set_analysis(db: &Path, literal_sql: &str) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let n = conn
        .execute(
            &format!(
                "UPDATE generations SET analysis_json = {literal_sql}
                  WHERE id = (SELECT max(id) FROM generations)"
            ),
            [],
        )
        .unwrap();
    assert_eq!(n, 1);
}

fn analysis(db: &Path) -> String {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT analysis_json FROM generations WHERE id = (SELECT max(id) FROM generations)",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

/// Attack 2: a stamp of the wrong JSON TYPE is not the string the writer wrote.
/// `CAST(json_extract(..) AS TEXT)` turns the number 2 into "2", so a number
/// accepted as the current epoch is a stamp nobody wrote.
#[test]
fn a_numeric_stamp_equal_to_the_epoch_text_is_not_current() {
    let (root, db, store) = indexed("number");
    set_analysis(
        &db,
        &format!("json_set(analysis_json, '$.resolver_epoch', json('{RESOLUTION_REUSE_EPOCH}'))"),
    );
    assert!(
        !store.latest_generation_payload_is_current().unwrap(),
        "number {RESOLUTION_REUSE_EPOCH} is not the string stamp the writer produces"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_boolean_or_nested_or_padded_stamp_is_not_current() {
    for (label, value) in [
        ("padded", format!("json_quote(' {RESOLUTION_REUSE_EPOCH} ')")),
        ("bool", "json('true')".to_string()),
        ("object", format!("json('{{\"v\":\"{RESOLUTION_REUSE_EPOCH}\"}}')")),
        ("array", format!("json('[\"{RESOLUTION_REUSE_EPOCH}\"]')")),
        ("null", "json('null')".to_string()),
    ] {
        let (root, db, store) = indexed(label);
        set_analysis(
            &db,
            &format!("json_set(analysis_json, '$.resolver_epoch', {value})"),
        );
        assert!(
            !store.latest_generation_payload_is_current().unwrap(),
            "{label} stamp must not count as current"
        );
        let _ = fs::remove_dir_all(&root);
    }
}

/// Attack 2: an ambiguous document (duplicate key) must fail closed whichever
/// duplicate SQLite resolves to.
#[test]
fn a_duplicate_stamp_key_never_reads_as_current() {
    for (label, doc) in [
        (
            "dup-current-first",
            format!("'{{\"resolver_epoch\":\"{RESOLUTION_REUSE_EPOCH}\",\"resolver_epoch\":\"0\"}}'"),
        ),
        (
            "dup-current-last",
            format!("'{{\"resolver_epoch\":\"0\",\"resolver_epoch\":\"{RESOLUTION_REUSE_EPOCH}\"}}'"),
        ),
    ] {
        let (root, db, store) = indexed(label);
        set_analysis(&db, &doc);
        assert!(
            !store.latest_generation_payload_is_current().unwrap(),
            "{label}: contradictory stamps are not a passing stamp"
        );
        let _ = fs::remove_dir_all(&root);
    }
}

/// Attack 2/5: the stored stamp is echoed verbatim into `degraded_reason`. An
/// oversized stamp must not become an oversized status field, and a stamp must
/// not be able to spell the phrase that makes SessionStart rebuild
/// (`rebuild_required_reason` matches "stored extraction payload is obsolete").
#[test]
fn a_hostile_stamp_is_not_echoed_unbounded_or_as_a_rebuild_trigger() {
    let (root, db, store) = indexed("hostile");
    let huge = "9".repeat(1_000_000);
    set_analysis(
        &db,
        &format!("json_set(analysis_json, '$.resolver_epoch', '{huge}')"),
    );
    let reason = store
        .status("test")
        .unwrap()
        .degraded_reason
        .expect("a reason is named");
    assert!(reason.len() < 1024, "status echoed {} bytes", reason.len());

    set_analysis(
        &db,
        "json_set(analysis_json, '$.resolver_epoch', 'stored extraction payload is obsolete')",
    );
    let reason = store
        .status("test")
        .unwrap()
        .degraded_reason
        .expect("a reason is named");
    assert!(
        !reason.contains("stored extraction payload is obsolete"),
        "a stamp spelled the rebuild trigger: {reason}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// Attack 2: a stamp NEWER than this binary. Reading must not rewrite the
/// store, must not loop, and the status must say the store is newer, not that
/// it is "older" or that this binary should rebuild it down.
#[test]
fn a_newer_stamp_is_reported_as_newer_and_reading_does_not_rewrite() {
    let (root, db, store) = indexed("newer");
    set_analysis(&db, "json_set(analysis_json, '$.resolver_epoch', '999')");
    let before = analysis(&db);
    assert!(!store.latest_generation_payload_is_current().unwrap());
    let _ = store.status("test").unwrap();
    let _ = store.status("test").unwrap();
    assert_eq!(analysis(&db), before, "reads never rewrite analysis_json");
    let reason = store.status("test").unwrap().degraded_reason.unwrap();
    assert!(
        reason.to_lowercase().contains("newer"),
        "a newer-epoch store must be named newer, not asked to rebuild down: {reason}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// Attack 3: one re-resolve heals; the next zero-edit write is current, and a
/// head restamp or a prune neither launders a stale stamp nor strips a good one.
#[test]
fn a_rewrite_restores_current_and_restamp_does_not_launder_a_stale_stamp() {
    let (root, db, store) = indexed("heal");
    set_analysis(&db, "json_set(analysis_json, '$.resolver_epoch', '1')");
    store.restamp_latest_head_keeping_seed_rebind("abc123").unwrap();
    assert!(
        !store.latest_generation_payload_is_current().unwrap(),
        "restamping the head must not make stale edges current"
    );
    write_generation(&root, &store);
    assert!(store.latest_generation_payload_is_current().unwrap());
    store.restamp_latest_head_keeping_seed_rebind("def456").unwrap();
    assert!(store.latest_generation_payload_is_current().unwrap());
    store.prune_generations_except_latest(1).unwrap();
    assert!(store.latest_generation_payload_is_current().unwrap());
    let _ = fs::remove_dir_all(&root);
}

/// Attack 1: a store reopened (migration path) keeps a stale stamp stale.
#[test]
fn reopening_a_store_does_not_launder_a_missing_stamp() {
    let (root, db, store) = indexed("reopen");
    set_analysis(&db, "json_remove(analysis_json, '$.resolver_epoch')");
    drop(store);
    let reopened = Store::open(&db).unwrap();
    assert!(!reopened.latest_generation_payload_is_current().unwrap());
    let _ = fs::remove_dir_all(&root);
}

/// Attack 7: the stamped document still parses as the analysis summary with
/// unknown keys rejected at the top level of nothing in this repo; assert the
/// summary reads back and the stamp is a top-level string.
#[test]
fn the_stamped_document_reads_back_as_an_analysis_summary() {
    let (root, db, store) = indexed("backcompat");
    let doc: serde_json::Value = serde_json::from_str(&analysis(&db)).unwrap();
    assert_eq!(doc["resolver_epoch"], serde_json::json!(RESOLUTION_REUSE_EPOCH));
    assert!(store.latest_analysis().unwrap().is_some());
    let _ = fs::remove_dir_all(&root);
}
