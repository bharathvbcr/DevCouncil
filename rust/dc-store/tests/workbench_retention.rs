use dc_store::Store;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

fn call(s: &Store, method: &str, raw: &str) -> String {
    s.workbench_request(method, raw)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn text(s: &Store, raw: &str, path: &str) -> String {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn num(s: &Store, raw: &str, path: &str) -> i64 {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn fixture() -> (Store, Arc<AtomicI64>) {
    let mut s = Store::open_in_memory().unwrap();
    let clock = Arc::new(AtomicI64::new(1000));
    let c = clock.clone();
    s.set_clock(move || c.load(Ordering::SeqCst));
    let root = if cfg!(windows) {
        "C:/checkout"
    } else {
        "/checkout"
    };
    call(
        &s,
        "repositories.put",
        &format!(
            r#"{{"id":"r","request_id":"r","expected_revision":0,"name":"Repo","identity_key":"local:{root}/.git"}}"#
        ),
    );
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"t","expected_revision":0,"title":"Task","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    call(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"run","request_id":"prep","expected_revision":0,"task_id":"t","source_revision":1,"repository_id":"r","repository_revision":1,"provider":"codex","permission_mode":"ask","cwd":"{root}","git_dir":"{root}/.git","git_common_dir":"{root}/.git","head_oid":null}}"#
        ),
    );
    call(
        &s,
        "runs.claim",
        r#"{"id":"run","request_id":"claim-run","expected_revision":1,"owner_id":"host","session_id":"session"}"#,
    );
    call(
        &s,
        "runs.started",
        r#"{"id":"run","request_id":"started","expected_revision":2,"owner_id":"host","session_id":"session","process_id":123,"process_start":"observed-birth"}"#,
    );
    (s, clock)
}
const CREATE: &str = r#"{"id":"decision","request_id":"create","expected_revision":0,"run_id":"run","owner_id":"host","session_id":"session","provider_thread_id":"thread","provider_turn_id":"turn","protocol_request_id":"n:42","kind":"permission","payload":"{\"command\":\"git status\"}","payload_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","deadline":1400}"#;
const DECIDE: &str = r#"{"id":"decision","request_id":"decide","expected_revision":1,"payload_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","decision":"allow_once"}"#;
const CLAIM: &str = r#"{"id":"decision","request_id":"deliver","expected_revision":2,"owner_id":"host","session_id":"session","provider_thread_id":"thread","provider_turn_id":"turn","protocol_request_id":"n:42","payload_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

const DAY: i64 = 86_400;
fn count(s: &Store, table: &str) -> i64 {
    s.connection()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn compact(s: &Store, raw: &str) -> String {
    call(s, "store.compact", raw)
}
fn put(s: &Store, n: i64) -> Result<String, dc_store::workbench::Error> {
    s.workbench_request(
        "items.put",
        &format!(
            r#"{{"id":"t","request_id":"put-{n}","expected_revision":{n},"title":"Task {n}","repository_ids":["r"],"primary_repository_id":"r"}}"#
        ),
    )
}
// Revision 1 of "t" exists from the fixture; revisions 2..=n+1 follow.
fn churn(s: &Store, times: i64) {
    for n in 1..=times {
        put(s, n).unwrap_or_else(|e| panic!("put {n}: {e}"));
    }
}

#[test]
fn receipt_inside_the_window_replays_and_one_outside_is_pruned() {
    let (s, time) = fixture();
    churn(&s, 3);
    let original = put(&s, 3).unwrap();
    time.fetch_add(8 * DAY, Ordering::SeqCst);
    // A fresh mutation now, so the newest receipt is inside any window.
    let fresh = call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"fresh","expected_revision":4,"title":"Fresh","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    let report = compact(
        &s,
        r#"{"receipt_window_seconds":604800,"event_retention_seconds":2592000}"#,
    );
    assert!(num(&s, &report, "$.item.receipts.deleted") > 0);
    // Inside the window: byte-identical receipt, no second revision.
    assert_eq!(
        s.workbench_request(
            "items.put",
            r#"{"id":"t","request_id":"fresh","expected_revision":4,"title":"Fresh","repository_ids":["r"],"primary_repository_id":"r"}"#
        )
        .unwrap(),
        fresh
    );
    // Outside the window: the receipt is gone, so the same request id is judged
    // by expected_revision (now stale) instead of replaying.
    assert_eq!(put(&s, 3).unwrap_err().code, "revision_conflict");
    assert!(!original.is_empty());
}

#[test]
fn launch_claim_receipts_are_never_pruned() {
    let (s, time) = fixture();
    time.fetch_add(365 * DAY, Ordering::SeqCst);
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"later","expected_revision":1,"title":"Later","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    compact(&s, r#"{"keep_events":1}"#);
    let kept: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_requests WHERE method='runs.claim'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
    assert_eq!(
        s.workbench_request(
            "runs.claim",
            r#"{"id":"run","request_id":"claim-run","expected_revision":1,"owner_id":"host","session_id":"session"}"#
        )
        .unwrap_err()
        .code,
        "claim_consumed"
    );
}

#[test]
fn revision_bound_holds_and_expected_revision_still_decides_writes() {
    let (s, _) = fixture();
    churn(&s, 40);
    let report = compact(&s, r#"{"keep_revisions":5}"#);
    assert_eq!(num(&s, &report, "$.item.revisions.truncated"), 0);
    let kept: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_revisions WHERE entity_type='item' AND entity_id='t'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 5);
    let history = call(&s, "items.history", r#"{"id":"t","limit":200}"#);
    assert_eq!(num(&s, &history, "$.total"), 5);
    assert_eq!(num(&s, &history, "$.items[4].revision"), 41);
    // The live row's revision is intact: a stale write still conflicts and the
    // current one still lands.
    let stale = s.workbench_request(
        "items.put",
        r#"{"id":"t","request_id":"stale","expected_revision":40,"title":"Stale","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    assert_eq!(stale.unwrap_err().code, "revision_conflict");
    assert_eq!(num(&s, &put(&s, 41).unwrap(), "$.item.revision"), 42);
}

#[test]
fn open_decisions_and_live_runs_keep_every_revision() {
    let (s, _) = fixture();
    call(&s, "decisions.create", CREATE);
    call(&s, "decisions.decide", DECIDE); // state `decided` is still open
    let before = count(&s, "work_revisions");
    let report = compact(&s, r#"{"keep_revisions":1}"#);
    let run_revisions: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_revisions WHERE entity_type='run'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let decision_revisions: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_revisions WHERE entity_type='decision'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        run_revisions, 3,
        "prepare, claim and started of a running run"
    );
    assert_eq!(
        decision_revisions, 2,
        "create and decide of an open decision"
    );
    assert_eq!(
        count(&s, "work_revisions"),
        before - num(&s, &report, "$.item.revisions.deleted")
    );
    assert_eq!(count(&s, "work_decisions"), 1);
    assert_eq!(count(&s, "work_runs"), 1);
    // The open decision is still actionable end to end.
    let sent = call(&s, "decisions.claim", CLAIM);
    assert_eq!(text(&s, &sent, "$.item.state"), "dispatching");
}

#[test]
fn events_keep_the_newest_window_receipt_targets_and_attention_sources() {
    let (s, time) = fixture();
    churn(&s, 30);
    time.fetch_add(40 * DAY, Ordering::SeqCst);
    // Receipts expire, so only the newest-N and attention sources protect events.
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"late","expected_revision":31,"title":"Late","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    let attention_sources: i64 = s
        .connection()
        .query_row("SELECT count(*) FROM work_attention", [], |r| r.get(0))
        .unwrap();
    let max_before: i64 = s
        .connection()
        .query_row("SELECT max(sequence) FROM work_events", [], |r| r.get(0))
        .unwrap();
    let report = compact(&s, r#"{"keep_events":3}"#);
    assert!(num(&s, &report, "$.item.events.deleted") > 0);
    let max_after: i64 = s
        .connection()
        .query_row("SELECT max(sequence) FROM work_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        max_before, max_after,
        "the newest event is the cursor and survives"
    );
    let orphan_receipts: i64 = s.connection().query_row("SELECT count(*) FROM work_requests r WHERE NOT EXISTS(SELECT 1 FROM work_events e WHERE e.sequence=r.sequence)", [], |r| r.get(0)).unwrap();
    assert_eq!(orphan_receipts, 0, "no receipt points at a deleted event");
    let orphan_attention: i64 = s.connection().query_row("SELECT count(*) FROM work_attention a WHERE NOT EXISTS(SELECT 1 FROM work_events e WHERE e.sequence=a.source_sequence)", [], |r| r.get(0)).unwrap();
    assert_eq!(orphan_attention, 0);
    assert_eq!(count(&s, "work_attention"), attention_sources);
    // The event log still pages and its cursor keeps advancing past the gap.
    let page = call(&s, "events.list", r#"{"after":0,"limit":200}"#);
    assert!(num(&s, &page, "$.total") >= 3);
}

#[test]
fn one_pass_is_bounded_and_reports_what_it_left() {
    let (s, _) = fixture();
    churn(&s, 30);
    let first = compact(&s, r#"{"keep_revisions":1,"limit":10}"#);
    assert_eq!(num(&s, &first, "$.item.revisions.deleted"), 10);
    assert_eq!(num(&s, &first, "$.item.revisions.eligible"), 30);
    assert_eq!(num(&s, &first, "$.item.revisions.truncated"), 1);
    let rest = compact(&s, r#"{"keep_revisions":1,"limit":100}"#);
    assert_eq!(num(&s, &rest, "$.item.revisions.deleted"), 20);
    assert_eq!(
        num(
            &s,
            &compact(&s, r#"{"keep_revisions":1}"#),
            "$.item.revisions.deleted"
        ),
        0
    );
}

#[test]
fn unsafe_parameters_are_refused() {
    let (s, _) = fixture();
    for raw in [
        r#"{"receipt_window_seconds":59}"#,
        r#"{"receipt_window_seconds":86400,"event_retention_seconds":3600}"#,
        r#"{"keep_events":0}"#,
        r#"{"keep_revisions":0}"#,
        r#"{"limit":0}"#,
        r#"{"limit":100001}"#,
        r#"{"unknown":1}"#,
    ] {
        assert!(s.workbench_request("store.compact", raw).is_err(), "{raw}");
    }
}
