//! The workbench store at scale: one-request lookups, receipts that do not
//! copy a record's body, bounded page totals, and an auto_vacuum profile.

use dc_store::Store;

mod support;

fn call(s: &Store, method: &str, raw: &str) -> String {
    s.workbench_request(method, raw)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn json<T: rusqlite::types::FromSql>(s: &Store, raw: &str, path: &str) -> T {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn repo(s: &Store, n: usize) {
    call(
        s,
        "repositories.put",
        &format!(
            r#"{{"id":"r{n}","request_id":"r{n}","expected_revision":0,"name":"Repo {n}","identity_key":"local:/checkout-{n}/.git"}}"#
        ),
    );
}
fn put(s: &Store, n: i64, title: &str) -> String {
    call(
        s,
        "items.put",
        &format!(
            r#"{{"id":"t","request_id":"put-{n}","expected_revision":{n},"title":"{title}","repository_ids":["r0"],"primary_repository_id":"r0"}}"#
        ),
    )
}
fn store_with_task(revisions: i64) -> Store {
    let s = Store::open_in_memory().unwrap();
    repo(&s, 0);
    for n in 0..revisions {
        put(&s, n, &format!("Task {n}"));
    }
    s
}

#[test]
fn a_repository_is_found_by_identity_in_one_request() {
    let s = Store::open_in_memory().unwrap();
    for n in 0..3 {
        repo(&s, n);
    }
    let got = call(
        &s,
        "repositories.get",
        r#"{"identity_key":"local:/checkout-2/.git"}"#,
    );
    assert_eq!(json::<String>(&s, &got, "$.item.id"), "r2");
    let missing = s
        .workbench_request(
            "repositories.get",
            r#"{"identity_key":"local:/nowhere/.git"}"#,
        )
        .unwrap_err();
    assert_eq!(missing.code, "not_found");
    let both = s
        .workbench_request(
            "repositories.get",
            r#"{"id":"r1","identity_key":"local:/checkout-1/.git"}"#,
        )
        .unwrap_err();
    assert_eq!(both.code, "invalid_input");
    // The id form still works.
    let by_id = call(&s, "repositories.get", r#"{"id":"r1"}"#);
    assert_eq!(
        json::<String>(&s, &by_id, "$.item.identity_key"),
        "local:/checkout-1/.git"
    );
}

#[test]
fn history_newest_first_reads_the_last_revision_in_one_request() {
    let s = store_with_task(30);
    let last = call(
        &s,
        "items.history",
        r#"{"id":"t","limit":1,"newest_first":true}"#,
    );
    assert_eq!(json::<i64>(&s, &last, "$.shown"), 1);
    assert_eq!(json::<i64>(&s, &last, "$.items[0].revision"), 30);
    // Paging down continues below the cursor, newest to oldest.
    let cursor: i64 = json(&s, &last, "$.next_cursor");
    let next = call(
        &s,
        "items.history",
        &format!(r#"{{"id":"t","limit":2,"newest_first":true,"before_revision":{cursor}}}"#),
    );
    assert_eq!(json::<i64>(&s, &next, "$.items[0].revision"), 29);
    assert_eq!(json::<i64>(&s, &next, "$.items[1].revision"), 28);
    // Ascending is unchanged, and mixing the two cursors is refused.
    let first = call(&s, "items.history", r#"{"id":"t","limit":1}"#);
    assert_eq!(json::<i64>(&s, &first, "$.items[0].revision"), 1);
    for raw in [
        r#"{"id":"t","newest_first":true,"after_revision":3}"#,
        r#"{"id":"t","before_revision":3}"#,
    ] {
        assert_eq!(
            s.workbench_request("items.history", raw).unwrap_err().code,
            "invalid_input"
        );
    }
}

#[test]
fn a_receipt_does_not_copy_the_body_and_still_replays_byte_for_byte() {
    let s = Store::open_in_memory().unwrap();
    repo(&s, 0);
    let big = "x".repeat(60_000);
    let raw = format!(
        r#"{{"id":"t","request_id":"big","expected_revision":0,"title":"Big","description":"{big}","repository_ids":["r0"],"primary_repository_id":"r0"}}"#
    );
    let original = call(&s, "items.put", &raw);
    let stored: i64 = s
        .connection()
        .query_row(
            "SELECT length(response) FROM work_requests WHERE id='big'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        stored < 1_000,
        "the receipt kept {stored} bytes of a body the event already holds"
    );
    assert_eq!(
        call(&s, "items.put", &raw),
        original,
        "replay must equal the first response"
    );
    assert_eq!(
        json::<i64>(&s, &original, "$.sequence"),
        json::<i64>(&s, &call(&s, "items.put", &raw), "$.sequence")
    );
    // Every other mutation kind replays the same way.
    let del = r#"{"id":"t","request_id":"del","expected_revision":1}"#;
    let first = call(&s, "items.delete", del);
    assert_eq!(call(&s, "items.delete", del), first);
}

#[test]
fn a_receipt_whose_event_is_gone_fails_loudly() {
    let s = store_with_task(1);
    let raw = r#"{"id":"t","request_id":"put-0","expected_revision":0,"title":"Task 0","repository_ids":["r0"],"primary_repository_id":"r0"}"#;
    // Break the invariant retention maintains (receipts are deleted before their events).
    s.connection().execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM work_events WHERE sequence=(SELECT sequence FROM work_requests WHERE id='put-0')").unwrap();
    assert_eq!(
        s.workbench_request("items.put", raw).unwrap_err().code,
        "receipt_unreadable"
    );
}

#[test]
fn page_totals_are_exact_on_a_last_page_and_flagged_when_counted_past_a_page() {
    let s = store_with_task(5);
    let all = call(&s, "events.list", "{}");
    assert_eq!(
        json::<i64>(&s, &all, "$.total"),
        json::<i64>(&s, &all, "$.shown")
    );
    assert_eq!(json::<i64>(&s, &all, "$.total_exact"), 1);
    let partial = call(&s, "events.list", r#"{"limit":2}"#);
    assert_eq!(json::<i64>(&s, &partial, "$.total"), 6);
    assert_eq!(json::<i64>(&s, &partial, "$.total_exact"), 1);
    let history = call(&s, "items.history", r#"{"id":"t","limit":2}"#);
    assert_eq!(json::<i64>(&s, &history, "$.total"), 5);
}

#[test]
fn a_new_profile_reclaims_pages_when_retention_frees_them() {
    let db = support::temp_dir("scale-vacuum").join("state.sqlite");
    let s = Store::open(&db).unwrap();
    let mode: i64 = s
        .connection()
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, 2, "a created profile is auto_vacuum=INCREMENTAL");
    repo(&s, 0);
    for n in 0..40 {
        let body = "y".repeat(40_000);
        call(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"t","request_id":"v-{n}","expected_revision":{n},"title":"T","description":"{body}","repository_ids":["r0"],"primary_repository_id":"r0"}}"#
            ),
        );
    }
    let report = call(
        &s,
        "store.compact",
        r#"{"keep_revisions":1,"keep_events":1,"receipt_window_seconds":3600,"event_retention_seconds":3600}"#,
    );
    // Nothing is old enough yet, so nothing to free: the report still says what vacuum did.
    assert_eq!(json::<i64>(&s, &report, "$.item.vacuum.incremental"), 1);
    assert!(json::<i64>(&s, &report, "$.item.revisions.deleted") >= 39);
    assert_eq!(
        json::<i64>(&s, &report, "$.item.vacuum.free_pages_after"),
        0,
        "{report}"
    );
}

/// Bytes in the WAL sidecar. A header is 32 bytes; anything longer is a frame.
/// The receipt may describe freelist progress only after those frames are gone.
fn wal_bytes(db: &std::path::Path) -> u64 {
    let wal = format!("{}-wal", db.display());
    match std::fs::metadata(&wal) {
        Ok(meta) => meta.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("stat {wal}: {error}"),
    }
}

/// Freelist count stored in the main database header. Checkpoint is what
/// copies a WAL frame into this header; a read through the connection would
/// count pages the WAL still holds.
fn header_freelist(db: &std::path::Path) -> i64 {
    let bytes = std::fs::read(db).unwrap_or_else(|error| panic!("open {}: {error}", db.display()));
    assert!(
        bytes.len() >= 40,
        "database header is short ({})",
        bytes.len()
    );
    let raw = u32::from_be_bytes(bytes[36..40].try_into().expect("four header bytes"));
    i64::from(raw)
}

fn assert_receipt_matches_wal(db: &std::path::Path, s: &Store, report: &str) {
    let incremental = json::<i64>(s, report, "$.item.vacuum.incremental");
    let before = json::<i64>(s, report, "$.item.vacuum.free_pages_before");
    let after = json::<i64>(s, report, "$.item.vacuum.free_pages_after");
    let wal = wal_bytes(db);
    let on_disk = header_freelist(db);
    if wal > 32 {
        assert_eq!(
            incremental, 0,
            "incremental vacuum was claimed while {wal} WAL bytes still hold pages\n{report}"
        );
        assert_eq!(
            before, after,
            "freelist moved from {before} to {after} while the WAL still holds pages\n{report}"
        );
        assert!(
            before <= on_disk,
            "receipt freelist {before} is above the main-file header {on_disk}; the extra pages are still in the WAL\n{report}"
        );
    } else {
        assert_eq!(
            after, on_disk,
            "checkpoint left no WAL frames, so free_pages_after must be the main-file header ({on_disk})\n{report}"
        );
    }
}

#[test]
fn a_wal_frame_is_not_reported_as_freelist_progress() {
    let dir = support::temp_dir("wal-receipt");
    let db = dir.join("state.sqlite");
    let s = Store::open(&db).unwrap();
    repo(&s, 0);
    for n in 0..20 {
        let body = "y".repeat(16_000);
        call(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"t","request_id":"v-{n}","expected_revision":{n},"title":"T","description":"{body}","repository_ids":["r0"],"primary_repository_id":"r0"}}"#
            ),
        );
    }
    let wal_before = wal_bytes(&db);
    assert!(
        wal_before > 32,
        "the fixture must leave a WAL frame, got {wal_before} bytes"
    );
    let report = call(
        &s,
        "store.compact",
        r#"{"keep_revisions":1,"keep_events":1,"receipt_window_seconds":3600,"event_retention_seconds":3600}"#,
    );
    assert!(
        json::<i64>(&s, &report, "$.item.revisions.deleted") >= 19,
        "{report}"
    );
    assert!(
        wal_bytes(&db) <= 32,
        "no reader was holding the WAL, so checkpoint should have copied every frame\n{report}"
    );
    assert_eq!(
        json::<String>(&s, &report, "$.item.vacuum.checkpoint"),
        "cleared",
        "{report}"
    );
    assert_eq!(
        json::<i64>(&s, &report, "$.item.vacuum.incremental"),
        1,
        "{report}"
    );
    assert_receipt_matches_wal(&db, &s, &report);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reader_blocking_checkpoint_is_not_told_the_wal_pages_were_freed() {
    let dir = support::temp_dir("wal-receipt-blocked");
    let db = dir.join("state.sqlite");
    let s = Store::open(&db).unwrap();
    repo(&s, 0);
    for n in 0..20 {
        let body = "y".repeat(16_000);
        call(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"t","request_id":"b-{n}","expected_revision":{n},"title":"T","description":"{body}","repository_ids":["r0"],"primary_repository_id":"r0"}}"#
            ),
        );
    }
    assert!(wal_bytes(&db) > 32, "the fixture must leave a WAL frame");
    let reader = Store::open(&db).unwrap();
    reader
        .connection()
        .execute_batch("BEGIN DEFERRED; SELECT count(*) FROM work_revisions;")
        .unwrap();
    let started = std::time::Instant::now();
    let report = call(
        &s,
        "store.compact",
        r#"{"keep_revisions":1,"keep_events":1,"receipt_window_seconds":3600,"event_retention_seconds":3600}"#,
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "a blocked checkpoint waited {:?}; the bound is 250ms per attempt",
        started.elapsed()
    );
    assert!(
        json::<i64>(&s, &report, "$.item.revisions.deleted") >= 19,
        "a blocked checkpoint must not roll the deletes back\n{report}"
    );
    assert!(
        wal_bytes(&db) > 32,
        "the open reader should have left a WAL frame for the receipt to refuse\n{report}"
    );
    assert_eq!(
        json::<String>(&s, &report, "$.item.vacuum.checkpoint"),
        "blocked",
        "{report}"
    );
    assert_receipt_matches_wal(&db, &s, &report);
    drop(reader);
    let _ = std::fs::remove_dir_all(&dir);
}

fn stored_version(s: &Store) -> i64 {
    s.connection()
        .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
        .unwrap()
}

/// A build that knows schema 12 replays whatever a receipt stores as the
/// response and refuses any profile whose version is not 12. A short receipt
/// replayed that way would answer a retry with a response that has no record
/// in it, so every profile the new code writes short receipts into must carry
/// a version that build refuses.
#[test]
fn a_profile_holding_short_receipts_is_stamped_past_what_schema_twelve_accepts() {
    let s = store_with_task(1);
    let stored: String = s
        .connection()
        .query_row(
            "SELECT response FROM work_requests WHERE id='put-0'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(json::<Option<String>>(&s, &stored, "$.replay").is_some());
    assert!(json::<Option<String>>(&s, &stored, "$.item").is_none());
    assert_eq!(dc_store::workbench::WORKBENCH_SCHEMA, 16);
    assert_ne!(
        stored_version(&s),
        12,
        "a schema 12 host would replay the short receipt as a response"
    );
}

#[test]
fn an_upgraded_schema_twelve_profile_still_replays_its_full_receipts() {
    let s = store_with_task(1);
    let raw = r#"{"id":"t","request_id":"put-0","expected_revision":0,"title":"Task 0","repository_ids":["r0"],"primary_repository_id":"r0"}"#;
    let original = call(&s, "items.put", raw);
    // Rewrite the receipt as schema 12 stored it: the whole response.
    s.connection()
        .execute(
            "UPDATE work_requests SET response=?1 WHERE id='put-0'",
            [&original],
        )
        .unwrap();
    s.connection()
        .execute_batch("UPDATE work_meta SET version=12 WHERE id=1")
        .unwrap();
    s.upgrade_workbench_profile().unwrap();
    assert_eq!(stored_version(&s), 16);
    assert_eq!(call(&s, "items.put", raw), original);
}
