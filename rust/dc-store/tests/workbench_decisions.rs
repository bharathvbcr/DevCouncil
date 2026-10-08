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

#[test]
fn decision_is_durable_bounded_and_one_use_without_accepting_the_task() {
    let (s, _) = fixture();
    let created = call(&s, "decisions.create", CREATE);
    assert_eq!(num(&s, &created, "$.item.expires_at"), 1300);
    assert_eq!(text(&s, &created, "$.item.state"), "pending");
    assert_eq!(call(&s, "decisions.create", CREATE), created);
    call(&s, "decisions.decide", DECIDE);
    let sent = call(&s, "decisions.claim", CLAIM);
    assert_eq!(text(&s, &sent, "$.item.state"), "dispatching");
    assert_eq!(
        s.workbench_request("decisions.claim", CLAIM)
            .unwrap_err()
            .code,
        "claim_consumed"
    );
    assert_eq!(
        num(
            &s,
            &call(&s, "items.get", r#"{"id":"t"}"#),
            "$.item.revision"
        ),
        1
    );
    call(
        &s,
        "decisions.resolve",
        r#"{"id":"decision","request_id":"resolve","expected_revision":3,"owner_id":"host","session_id":"session","state":"resolved","reason":"Provider confirmed resolution"}"#,
    );
    assert_eq!(
        num(
            &s,
            &call(
                &s,
                "decisions.list",
                r#"{"run_id":"run","state":"pending","limit":1}"#
            ),
            "$.total"
        ),
        0
    );
}

#[test]
fn stale_identity_payload_task_and_deadline_cannot_grant_permission() {
    let (s, time) = fixture();
    call(&s, "decisions.create", CREATE);
    assert!(
        s.workbench_request("decisions.decide", &DECIDE.replace("aaaaaaaa", "bbbbbbbb"))
            .is_err()
    );
    call(&s, "decisions.decide", DECIDE);
    for (from, to) in [
        ("\"host\"", "\"other\""),
        ("\"session\"", "\"other\""),
        ("\"thread\"", "\"other\""),
        ("\"turn\"", "\"other\""),
        ("n:42", "n:43"),
    ] {
        assert!(
            s.workbench_request("decisions.claim", &CLAIM.replace(from, to))
                .is_err()
        );
    }
    time.store(1300, Ordering::SeqCst);
    assert_eq!(
        s.workbench_request("decisions.claim", CLAIM)
            .unwrap_err()
            .code,
        "decision_stale"
    );
    time.store(1000, Ordering::SeqCst);
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"edit","expected_revision":1,"title":"Changed task","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    assert_eq!(
        s.workbench_request("decisions.claim", CLAIM)
            .unwrap_err()
            .code,
        "decision_stale"
    );
    let row = call(&s, "decisions.get", r#"{"id":"decision"}"#);
    assert_eq!(num(&s, &row, "$.item.actionable"), 0);
}

#[test]
fn receipt_failure_rolls_back_the_permission_before_any_delivery() {
    let (s, _) = fixture();
    call(&s, "decisions.create", CREATE);
    call(&s, "decisions.decide", DECIDE);
    s.connection().execute_batch("CREATE TRIGGER reject_decision_receipt BEFORE INSERT ON work_requests WHEN NEW.method='decisions.claim' BEGIN SELECT RAISE(ABORT,'receipt unavailable'); END;").unwrap();
    assert!(s.workbench_request("decisions.claim", CLAIM).is_err());
    assert_eq!(
        text(
            &s,
            &call(&s, "decisions.get", r#"{"id":"decision"}"#),
            "$.item.state"
        ),
        "decided"
    );
    s.connection()
        .execute_batch("DROP TRIGGER reject_decision_receipt")
        .unwrap();
    call(&s, "decisions.claim", CLAIM);
}

#[test]
fn questions_require_answers_and_cannot_become_tool_approvals() {
    let (s, _) = fixture();
    call(
        &s,
        "decisions.create",
        &CREATE.replace("\"permission\"", "\"question\""),
    );
    assert!(s.workbench_request("decisions.decide", DECIDE).is_err());
    assert!(
        s.workbench_request("decisions.decide", &DECIDE.replace("allow_once", "answer"))
            .is_err()
    );
    let answer = DECIDE.replace(
        "\"allow_once\"",
        "\"answer\",\"answer\":\"Use the workspace tests\"",
    );
    call(&s, "decisions.decide", &answer);
    assert_eq!(
        text(&s, &call(&s, "decisions.claim", CLAIM), "$.item.answer"),
        "Use the workspace tests"
    );
}

#[test]
fn ended_run_and_duplicate_provider_request_cannot_create_new_authority() {
    let (s, _) = fixture();
    call(&s, "decisions.create", CREATE);
    let duplicate = CREATE
        .replace("\"decision\"", "\"second\"")
        .replace("\"create\"", "\"second\"");
    assert!(s.workbench_request("decisions.create", &duplicate).is_err());
    call(
        &s,
        "runs.finish",
        r#"{"id":"run","request_id":"finish","expected_revision":3,"owner_id":"host","session_id":"session","outcome":"exited","exit_code":0,"reason":"Exited"}"#,
    );
    assert_eq!(
        s.workbench_request("decisions.decide", DECIDE)
            .unwrap_err()
            .code,
        "decision_stale"
    );
    assert!(
        s.workbench_request("decisions.create", &duplicate.replace("n:42", "n:43"))
            .is_err()
    );
}

#[test]
fn request_notices_are_private_and_expire_before_native_delivery() {
    let (s, time) = fixture();
    call(
        &s,
        "notifications.settings.put",
        r#"{"id":"profile","request_id":"enable","expected_revision":1,"enabled":true}"#,
    );
    let created = call(&s, "decisions.create", CREATE);
    let sequence = num(&s, &created, "$.sequence");
    let notice = call(
        &s,
        "attention.get",
        &format!(r#"{{"id":"event-{sequence}"}}"#),
    );
    assert_eq!(
        text(&s, &notice, "$.item.title"),
        "Coding agent needs permission"
    );
    assert!(!notice.contains("git status"));
    assert_eq!(
        num(
            &s,
            &call(&s, "notifications.pending.list", r#"{"minute_of_day":600}"#),
            "$.total"
        ),
        1
    );
    call(
        &s,
        "attention.update",
        &format!(
            r#"{{"id":"event-{sequence}","request_id":"read","expected_revision":1,"action":"read"}}"#
        ),
    );
    assert_eq!(
        text(
            &s,
            &call(&s, "decisions.get", r#"{"id":"decision"}"#),
            "$.item.state"
        ),
        "pending"
    );
    call(
        &s,
        "attention.update",
        &format!(
            r#"{{"id":"event-{sequence}","request_id":"unread","expected_revision":2,"action":"unread"}}"#
        ),
    );
    time.store(1300, Ordering::SeqCst);
    assert_eq!(
        text(
            &s,
            &call(
                &s,
                "attention.get",
                &format!(r#"{{"id":"event-{sequence}"}}"#)
            ),
            "$.item.target_status"
        ),
        "changed"
    );
    assert_eq!(
        num(
            &s,
            &call(&s, "notifications.pending.list", r#"{"minute_of_day":600}"#),
            "$.total"
        ),
        0
    );
}

#[test]
fn capture_and_notice_fail_together_and_pending_callbacks_are_bounded() {
    let (s, _) = fixture();
    s.connection().execute_batch("CREATE TRIGGER fail_attention BEFORE INSERT ON work_attention BEGIN SELECT RAISE(ABORT,'inbox unavailable'); END").unwrap();
    assert!(s.workbench_request("decisions.create", CREATE).is_err());
    assert_eq!(
        num(
            &s,
            &call(&s, "decisions.list", r#"{"run_id":"run"}"#),
            "$.total"
        ),
        0
    );
    s.connection()
        .execute_batch("DROP TRIGGER fail_attention")
        .unwrap();
    for i in 0..33 {
        let input = CREATE
            .replace("\"decision\"", &format!("\"decision-{i}\""))
            .replace("\"create\"", &format!("\"create-{i}\""))
            .replace("n:42", &format!("n:{i}"));
        let result = s.workbench_request("decisions.create", &input);
        if i < 32 {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, "capacity_reached");
        }
    }
    let first = call(&s, "decisions.list", r#"{"run_id":"run","limit":1}"#);
    assert_eq!(num(&s, &first, "$.total"), 32);
    assert_eq!(num(&s, &first, "$.shown"), 1);
    assert_eq!(num(&s, &first, "$.has_more"), 1);
    let cursor = text(&s, &first, "$.next_cursor");
    let rest = call(
        &s,
        "decisions.list",
        &format!(r#"{{"run_id":"run","limit":32,"cursor":"{cursor}"}}"#),
    );
    assert_eq!(num(&s, &rest, "$.shown"), 31);
    assert_eq!(num(&s, &rest, "$.total"), 32);
}

#[test]
fn repository_revision_and_invalid_authority_fields_are_checked_before_a_decision() {
    let (s, _) = fixture();
    call(
        &s,
        "notifications.settings.put",
        r#"{"id":"profile","request_id":"enable","expected_revision":1,"enabled":true}"#,
    );
    let created = call(&s, "decisions.create", CREATE);
    let sequence = num(&s, &created, "$.sequence");
    for extra in [
        r#", "skip_permissions":true"#,
        r#", "answer":"unrequested""#,
    ] {
        assert!(
            s.workbench_request(
                "decisions.decide",
                &DECIDE.replace('}', &format!("{extra}}}"))
            )
            .is_err()
        );
    }
    let root = if cfg!(windows) {
        "C:/checkout"
    } else {
        "/checkout"
    };
    call(
        &s,
        "repositories.put",
        &format!(
            r#"{{"id":"r","request_id":"repo-change","expected_revision":1,"name":"Renamed","identity_key":"local:{root}/.git"}}"#
        ),
    );
    assert_eq!(
        s.workbench_request("decisions.decide", DECIDE)
            .unwrap_err()
            .code,
        "decision_stale"
    );
    assert_eq!(
        text(
            &s,
            &call(
                &s,
                "attention.get",
                &format!(r#"{{"id":"event-{sequence}"}}"#)
            ),
            "$.item.target_status"
        ),
        "changed"
    );
    assert_eq!(
        num(
            &s,
            &call(&s, "notifications.pending.list", r#"{"minute_of_day":600}"#),
            "$.total"
        ),
        0
    );
}

#[test]
fn schema_seven_upgrade_is_atomic_and_keeps_runs_and_settings() {
    let (s, _) = fixture();
    let run = call(&s, "runs.get", r#"{"id":"run"}"#);
    let settings = call(&s, "notifications.settings.get", r#"{"id":"profile"}"#);
    s.connection().execute_batch("DROP TABLE work_item_links; DROP INDEX work_items_archive; DROP INDEX work_items_completed; ALTER TABLE work_items DROP COLUMN archived; ALTER TABLE work_items DROP COLUMN completed_at; DROP TABLE work_decisions; UPDATE work_meta SET version=7; CREATE INDEX work_decisions_run_state ON work_runs(id);").unwrap();
    assert_eq!(
        s.workbench_request("decisions.list", r#"{"run_id":"run"}"#)
            .unwrap_err()
            .code,
        "store_error"
    );
    let version: i64 = s
        .connection()
        .query_row("SELECT version FROM work_meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 7);
    let tables: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='work_decisions'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    s.connection()
        .execute_batch("DROP INDEX work_decisions_run_state")
        .unwrap();
    assert_eq!(
        num(
            &s,
            &call(&s, "decisions.list", r#"{"run_id":"run"}"#),
            "$.total"
        ),
        0
    );
    assert_eq!(call(&s, "runs.get", r#"{"id":"run"}"#), run);
    assert_eq!(
        call(&s, "notifications.settings.get", r#"{"id":"profile"}"#),
        settings
    );
}

// Host-raised review decisions. The host passes the run's own owner and
// session, and synthetic provider identities: thread `host`, turn = the kind,
// request = the head commit.
const HEAD: &str = "2222222222222222222222222222222222222222";
const DIGEST: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const THIRTY_DAYS: i64 = 30 * 86_400;

fn review_payload(head: &str) -> String {
    format!(
        r#"{{"repository_id":"r","base_oid":"1111111111111111111111111111111111111111","head_oid":"{head}","branch":"agent/task","files_changed":3,"diff_digest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}}"#
    )
}
fn host_create_with(id: &str, kind: &str, head: &str, payload: &str) -> String {
    format!(
        r#"{{"id":"{id}","request_id":"create-{id}","expected_revision":0,"run_id":"run","owner_id":"host","session_id":"session","provider_thread_id":"host","provider_turn_id":"{kind}","protocol_request_id":"{head}","kind":"{kind}","payload":{},"payload_digest":"{DIGEST}","deadline":{}}}"#,
        serde_quote(payload),
        1000 + 40 * 86_400
    )
}
fn host_create(id: &str, kind: &str, head: &str) -> String {
    host_create_with(id, kind, head, &review_payload(head))
}
fn serde_quote(raw: &str) -> String {
    format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
}
fn host_decide(id: &str, revision: i64, decision: &str, note: Option<&str>) -> String {
    let note = note.map_or(String::new(), |n| {
        format!(r#","answer":{}"#, serde_quote(n))
    });
    format!(
        r#"{{"id":"{id}","request_id":"decide-{id}-{revision}","expected_revision":{revision},"payload_digest":"{DIGEST}","decision":"{decision}"{note}}}"#
    )
}
fn host_claim(id: &str, request: &str, revision: i64, kind: &str, head: &str) -> String {
    format!(
        r#"{{"id":"{id}","request_id":"{request}","expected_revision":{revision},"owner_id":"host","session_id":"session","provider_thread_id":"host","provider_turn_id":"{kind}","protocol_request_id":"{head}","payload_digest":"{DIGEST}"}}"#
    )
}
fn finish_run(s: &Store, outcome: &str) {
    let exit = if outcome == "exited" { "0" } else { "null" };
    call(
        s,
        "runs.finish",
        &format!(
            r#"{{"id":"run","request_id":"finish-{outcome}","expected_revision":3,"owner_id":"host","session_id":"session","outcome":"{outcome}","exit_code":{exit},"reason":"Ended"}}"#
        ),
    );
}
fn attention_total(s: &Store) -> i64 {
    num(
        s,
        &call(s, "attention.list", r#"{"filter":"all"}"#),
        "$.total",
    )
}

#[test]
fn change_review_is_created_only_for_an_ended_run_with_a_thirty_day_cap() {
    let (s, _) = fixture();
    let running = s
        .workbench_request(
            "decisions.create",
            &host_create("review", "change_review", HEAD),
        )
        .unwrap_err();
    assert_eq!(running.code, "invalid_state");
    finish_run(&s, "exited");
    let notices = attention_total(&s);
    let created = call(
        &s,
        "decisions.create",
        &host_create("review", "change_review", HEAD),
    );
    assert_eq!(text(&s, &created, "$.item.state"), "pending");
    assert_eq!(text(&s, &created, "$.item.kind"), "change_review");
    assert_eq!(num(&s, &created, "$.item.expires_at"), 1000 + THIRTY_DAYS);
    assert_eq!(num(&s, &created, "$.item.actionable"), 1);
    // Excluded from the inbox on purpose: the run's own exit notice already
    // points at the ended attempt, and consumers decode a closed kind set.
    assert_eq!(attention_total(&s), notices);
    let merge = call(
        &s,
        "decisions.create",
        &host_create("merge", "merge_unreviewed", HEAD),
    );
    assert_eq!(num(&s, &merge, "$.item.expires_at"), 1000 + THIRTY_DAYS);
    assert_eq!(attention_total(&s), notices);
    // One decision per kind per head per run.
    assert!(
        s.workbench_request(
            "decisions.create",
            &host_create("review", "change_review", HEAD)
                .replace("\"review\"", "\"again\"")
                .replace("create-review", "create-again"),
        )
        .is_err()
    );
}

#[test]
fn change_review_refuses_an_unresolved_or_never_claimed_run() {
    let (s, _) = fixture();
    finish_run(&s, "unresolved");
    assert_eq!(
        s.workbench_request(
            "decisions.create",
            &host_create("review", "change_review", HEAD)
        )
        .unwrap_err()
        .code,
        "invalid_state"
    );
    let root = if cfg!(windows) {
        "C:/checkout"
    } else {
        "/checkout"
    };
    call(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"never","request_id":"prep-never","expected_revision":0,"task_id":"t","source_revision":1,"repository_id":"r","repository_revision":1,"provider":"codex","permission_mode":"ask","cwd":"{root}/other","git_dir":"{root}/other/.git","git_common_dir":"{root}/.git","head_oid":null}}"#
        ),
    );
    call(
        &s,
        "runs.cancel",
        r#"{"id":"never","request_id":"cancel-never","expected_revision":1}"#,
    );
    assert!(
        s.workbench_request(
            "decisions.create",
            &host_create("review", "change_review", HEAD).replace("\"run\"", "\"never\"")
        )
        .is_err()
    );
}

#[test]
fn review_payload_is_validated_at_the_boundary() {
    let (s, _) = fixture();
    finish_run(&s, "exited");
    let good = review_payload(HEAD);
    let mut bad: Vec<String> = vec![
        good.replace(r#""files_changed":3,"#, ""),
        good.replace(r#""files_changed":3"#, r#""files_changed":-1"#),
        good.replace(r#""files_changed":3"#, r#""files_changed":"3""#),
        good.replace(r#""files_changed":3"#, r#""files_changed":3.5"#),
        good.replace("1111111111", "111111111A"),
        good.replace("1111111111111111111111111111111111111111", "111"),
        good.replace("cccccccccc", "cccccccc"),
        good.replace(r#""branch":"agent/task""#, r#""branch":"  ""#),
        good.replace(r#""branch":"agent/task""#, r#""branch":7"#),
        good.replace("agent/task", &"b".repeat(1025)),
        good.replace(r#""repository_id":"r""#, r#""repository_id":"elsewhere""#),
        good.replace('}', r#","extra":true}"#),
        "[]".to_owned(),
    ];
    bad.push(good.replace(r#""diff_digest""#, r#""digest""#));
    for payload in bad {
        assert!(
            s.workbench_request(
                "decisions.create",
                &host_create_with("review", "change_review", HEAD, &payload)
            )
            .is_err(),
            "accepted {payload}"
        );
    }
    // The synthetic provider identity binds the unique index to kind and head.
    for (from, to) in [
        (
            r#""provider_thread_id":"host""#,
            r#""provider_thread_id":"thread""#,
        ),
        (
            r#""provider_turn_id":"change_review""#,
            r#""provider_turn_id":"turn""#,
        ),
        (
            &format!(r#""protocol_request_id":"{HEAD}""#),
            r#""protocol_request_id":"n:42""#,
        ),
    ] {
        assert!(
            s.workbench_request(
                "decisions.create",
                &host_create("review", "change_review", HEAD).replace(from, to)
            )
            .is_err(),
            "accepted {to}"
        );
    }
    // A 64-hex (SHA-256 repository) head is accepted.
    let long = "3".repeat(64);
    call(
        &s,
        "decisions.create",
        &host_create("review", "change_review", &long),
    );
}

#[test]
fn change_review_decisions_carry_an_optional_bounded_note() {
    let (s, _) = fixture();
    finish_run(&s, "exited");
    for (i, (decision, note)) in [
        ("approve", Some("Looks right")),
        ("request_changes", None),
        ("deny", Some("Wrong approach")),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("review{i}");
        let head = format!("{i:040x}");
        call(
            &s,
            "decisions.create",
            &host_create(&id, "change_review", &head),
        );
        let decided = call(&s, "decisions.decide", &host_decide(&id, 1, decision, note));
        assert_eq!(text(&s, &decided, "$.item.state"), "decided");
        assert_eq!(text(&s, &decided, "$.item.decision"), decision);
        let answer: Option<String> = s
            .connection()
            .query_row("SELECT json_extract(?1,'$.item.answer')", [&decided], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(answer.as_deref(), note);
    }
    call(
        &s,
        "decisions.create",
        &host_create("long", "change_review", HEAD),
    );
    for note in ["x".repeat(4097), "   ".to_owned()] {
        assert!(
            s.workbench_request(
                "decisions.decide",
                &host_decide("long", 1, "approve", Some(&note))
            )
            .is_err()
        );
    }
    call(
        &s,
        "decisions.decide",
        &host_decide("long", 1, "approve", Some(&"x".repeat(4096))),
    );
}

#[test]
fn decisions_are_refused_for_the_wrong_kind() {
    let (s, _) = fixture();
    call(&s, "decisions.create", CREATE);
    for decision in ["approve", "request_changes"] {
        assert!(
            s.workbench_request("decisions.decide", &DECIDE.replace("allow_once", decision))
                .is_err()
        );
    }
    finish_run(&s, "exited");
    call(
        &s,
        "decisions.create",
        &host_create("review", "change_review", HEAD),
    );
    call(
        &s,
        "decisions.create",
        &host_create("merge", "merge_unreviewed", HEAD),
    );
    for (id, decision, note) in [
        ("review", "allow_once", Some("why")),
        ("review", "answer", Some("text")),
        ("merge", "approve", None),
        ("merge", "request_changes", None),
        ("merge", "answer", Some("text")),
    ] {
        assert!(
            s.workbench_request("decisions.decide", &host_decide(id, 1, decision, note))
                .is_err(),
            "{id} accepted {decision}"
        );
    }
}

#[test]
fn an_approved_change_review_is_never_consumed_by_claim() {
    let (s, _) = fixture();
    finish_run(&s, "exited");
    call(
        &s,
        "decisions.create",
        &host_create("review", "change_review", HEAD),
    );
    call(
        &s,
        "decisions.decide",
        &host_decide("review", 1, "approve", None),
    );
    let refused = s
        .workbench_request(
            "decisions.claim",
            &host_claim("review", "claim-review", 2, "change_review", HEAD),
        )
        .unwrap_err();
    assert_eq!(refused.code, "invalid_state");
    let row = call(&s, "decisions.get", r#"{"id":"review"}"#);
    assert_eq!(text(&s, &row, "$.item.state"), "decided");
    assert_eq!(text(&s, &row, "$.item.decision"), "approve");
}

#[test]
fn merge_unreviewed_needs_a_reason_and_is_claimed_exactly_once() {
    let (s, _) = fixture();
    finish_run(&s, "exited");
    call(
        &s,
        "decisions.create",
        &host_create("merge", "merge_unreviewed", HEAD),
    );
    for note in [None, Some("  ")] {
        assert!(
            s.workbench_request(
                "decisions.decide",
                &host_decide("merge", 1, "allow_once", note)
            )
            .is_err()
        );
    }
    call(
        &s,
        "decisions.decide",
        &host_decide("merge", 1, "allow_once", Some("Hotfix; reviewed offline")),
    );
    let claimed = call(
        &s,
        "decisions.claim",
        &host_claim("merge", "claim-merge", 2, "merge_unreviewed", HEAD),
    );
    assert_eq!(text(&s, &claimed, "$.item.state"), "dispatching");
    assert_eq!(
        s.workbench_request(
            "decisions.claim",
            &host_claim("merge", "claim-merge", 2, "merge_unreviewed", HEAD)
        )
        .unwrap_err()
        .code,
        "claim_consumed"
    );
    assert_eq!(
        s.workbench_request(
            "decisions.claim",
            &host_claim("merge", "claim-again", 3, "merge_unreviewed", HEAD)
        )
        .unwrap_err()
        .code,
        "decision_stale"
    );
    call(
        &s,
        "decisions.resolve",
        r#"{"id":"merge","request_id":"resolve-merge","expected_revision":3,"owner_id":"host","session_id":"session","state":"resolved","reason":"Merged"}"#,
    );
    // A denied override is never consumable as merge authority.
    let other = "4".repeat(40);
    call(
        &s,
        "decisions.create",
        &host_create("denied", "merge_unreviewed", &other),
    );
    call(
        &s,
        "decisions.decide",
        &host_decide("denied", 1, "deny", None),
    );
    assert_eq!(
        s.workbench_request(
            "decisions.claim",
            &host_claim("denied", "claim-denied", 2, "merge_unreviewed", &other)
        )
        .unwrap_err()
        .code,
        "invalid_state"
    );
}

#[test]
fn host_review_freshness_ignores_run_checkout_and_revisions_but_not_expiry() {
    let (s, time) = fixture();
    finish_run(&s, "exited");
    call(
        &s,
        "decisions.create",
        &host_create("review", "change_review", HEAD),
    );
    call(
        &s,
        "decisions.create",
        &host_create("merge", "merge_unreviewed", HEAD),
    );
    s.connection()
        .execute(
            "UPDATE work_runs SET body=json_set(body,'$.cwd','/elsewhere','$.permission_mode','bypass') WHERE id='run'",
            [],
        )
        .unwrap();
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"edit","expected_revision":1,"title":"Changed task","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    let root = if cfg!(windows) {
        "C:/checkout"
    } else {
        "/checkout"
    };
    call(
        &s,
        "repositories.put",
        &format!(
            r#"{{"id":"r","request_id":"repo-change","expected_revision":1,"name":"Renamed","identity_key":"local:{root}/.git"}}"#
        ),
    );
    let row = call(&s, "decisions.get", r#"{"id":"review"}"#);
    assert_eq!(num(&s, &row, "$.item.actionable"), 1);
    call(
        &s,
        "decisions.decide",
        &host_decide("review", 1, "approve", None),
    );
    // Past the 300 s live-callback cap the override is still decidable.
    time.store(1000 + 301, Ordering::SeqCst);
    call(
        &s,
        "decisions.decide",
        &host_decide("merge", 1, "allow_once", Some("Ship it")),
    );
    time.store(1000 + THIRTY_DAYS, Ordering::SeqCst);
    assert_eq!(
        s.workbench_request(
            "decisions.claim",
            &host_claim("merge", "claim-merge", 2, "merge_unreviewed", HEAD)
        )
        .unwrap_err()
        .code,
        "decision_stale"
    );
    let row = call(
        &s,
        "decisions.list",
        r#"{"run_id":"run","state":"decided"}"#,
    );
    assert_eq!(num(&s, &row, "$.total"), 2);
    assert_eq!(num(&s, &row, "$.items[0].actionable"), 0);
}

#[test]
fn host_reviews_share_the_live_decision_capacity_of_a_run() {
    let (s, _) = fixture();
    finish_run(&s, "exited");
    for i in 0..33 {
        let id = format!("review{i}");
        let result = s.workbench_request(
            "decisions.create",
            &host_create(&id, "change_review", &format!("{i:040x}")),
        );
        if i < 32 {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, "capacity_reached");
        }
    }
}
