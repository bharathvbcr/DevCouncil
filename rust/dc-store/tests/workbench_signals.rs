//! `signals.record`: a host watcher's CI result becomes one task-scoped inbox
//! notice through the ordinary attention path, with no delivery code of its own.
use dc_store::Store;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

const CWD: &str = if cfg!(windows) {
    "C:/checkout"
} else {
    "/checkout"
};
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn call(s: &Store, method: &str, input: &str) -> String {
    s.workbench_request(method, input)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn code(s: &Store, method: &str, input: &str) -> &'static str {
    s.workbench_request(method, input)
        .expect_err("the request should be refused")
        .code
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
fn absent(s: &Store, raw: &str, path: &str) -> bool {
    s.connection()
        .query_row("SELECT json_type(?1,?2) IS NULL", [raw, path], |r| r.get(0))
        .unwrap()
}
fn fixture() -> (Store, Arc<AtomicI64>) {
    let mut s = Store::open_in_memory().unwrap();
    let clock = Arc::new(AtomicI64::new(1000));
    let time = clock.clone();
    s.set_clock(move || time.load(Ordering::SeqCst));
    call(
        &s,
        "repositories.put",
        &format!(
            r#"{{"id":"r","request_id":"repo","expected_revision":0,"name":"Repo","identity_key":"local:{CWD}/.git"}}"#
        ),
    );
    for task in ["t", "u"] {
        call(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"{task}","request_id":"task-{task}","expected_revision":0,"title":"Private task {task}","repository_ids":["r"],"primary_repository_id":"r"}}"#
            ),
        );
    }
    (s, clock)
}
fn prepare(s: &Store, run: &str, task: &str) {
    let cwd = CWD;
    call(
        s,
        "runs.prepare",
        &format!(
            r#"{{"id":"{run}","request_id":"prepare-{run}","expected_revision":0,"task_id":"{task}","source_revision":1,"repository_id":"r","repository_revision":1,"provider":"codex","permission_mode":"ask","cwd":"{cwd}","git_dir":"{cwd}/.git","git_common_dir":"{cwd}/.git","head_oid":null}}"#
        ),
    );
}
/// A run that has ended, so nothing writes to it again.
fn ended(s: &Store, run: &str, task: &str) {
    prepare(s, run, task);
    call(
        s,
        "runs.claim",
        &format!(
            r#"{{"id":"{run}","request_id":"claim-{run}","expected_revision":1,"owner_id":"host","session_id":"session-{run}"}}"#
        ),
    );
    call(
        s,
        "runs.finish",
        &format!(
            r#"{{"id":"{run}","request_id":"finish-{run}","expected_revision":2,"owner_id":"host","session_id":"session-{run}","outcome":"failed","reason":"Failed before spawn"}}"#
        ),
    );
}
fn signal(request: &str, run: &str, task: &str, extra: &str) -> String {
    format!(
        r#"{{"request_id":"{request}","run_id":"{run}","task_id":"{task}","source":"github","external_id":"9001","kind":"ci_failed","conclusion":"failure","workflow":"CI","url":"https://github.com/o/r/actions/runs/9001","head_sha":"{SHA}"{extra}}}"#
    )
}
thread_local! {
    // SQLite's own JSON functions edit a request; no JSON dependency is added.
    static JSON: Store = Store::open_in_memory().unwrap();
}
/// `raw` with `key` set to a raw JSON fragment, or removed when it is `<drop>`.
fn replace(raw: &str, key: &str, value: &str) -> String {
    JSON.with(|s| {
        let path = format!("$.{key}");
        let sql = if value == "<drop>" {
            "SELECT json_remove(?1,?2)"
        } else {
            "SELECT json_set(?1,?2,json(?3))"
        };
        if value == "<drop>" {
            s.connection().query_row(sql, [raw, &path], |r| r.get(0))
        } else {
            s.connection()
                .query_row(sql, [raw, &path, value], |r| r.get(0))
        }
        .unwrap()
    })
}
fn events(s: &Store) -> String {
    call(s, "events.list", "{}")
}

#[test]
fn a_ci_result_becomes_one_run_notice_and_a_repeat_with_a_new_request_records_nothing() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let before = num(&s, &call(&s, "attention.list", "{}"), "$.total");
    let first = call(&s, "signals.record", &signal("poll-1", "run", "t", ""));
    assert!(num(&s, &first, "$.recorded") == 1);
    let id = text(&s, &first, "$.item.id");
    assert_eq!(id, format!("event-{}", num(&s, &first, "$.sequence")));
    assert_eq!(text(&s, &first, "$.item.kind"), "run_ci_failed");
    assert_eq!(
        text(&s, &first, "$.item.title"),
        "Coding agent branch failed CI"
    );
    assert_eq!(text(&s, &first, "$.item.task_id"), "t");
    assert_eq!(text(&s, &first, "$.item.target_type"), "run");
    assert_eq!(text(&s, &first, "$.item.target_id"), "run");
    assert_eq!(text(&s, &first, "$.item.target_status"), "current");
    assert_eq!(text(&s, &first, "$.item.signal.source"), "github");
    assert_eq!(text(&s, &first, "$.item.signal.external_id"), "9001");
    assert_eq!(text(&s, &first, "$.item.signal.conclusion"), "failure");
    assert_eq!(text(&s, &first, "$.item.signal.workflow"), "CI");
    assert_eq!(text(&s, &first, "$.item.signal.head_sha"), SHA);
    let page = call(&s, "attention.list", "{}");
    assert_eq!(num(&s, &page, "$.total"), before + 1);
    assert_eq!(text(&s, &page, "$.items[0].id"), id);
    assert!(!page.contains("Private task"));

    // The same request replays the same answer, byte for byte.
    assert_eq!(
        call(&s, "signals.record", &signal("poll-1", "run", "t", "")),
        first
    );
    // The same fact under a new request_id is a re-poll: nothing is recorded.
    let seen = events(&s);
    let again = call(&s, "signals.record", &signal("poll-2", "run", "t", ""));
    assert!(num(&s, &again, "$.recorded") == 0);
    assert_eq!(text(&s, &again, "$.item.id"), id);
    assert_eq!(num(&s, &again, "$.sequence"), num(&s, &first, "$.sequence"));
    assert_eq!(events(&s), seen);
    assert_eq!(
        num(&s, &call(&s, "attention.list", "{}"), "$.total"),
        before + 1
    );
    // The retry of that re-poll answers what it answered.
    assert_eq!(
        call(&s, "signals.record", &signal("poll-2", "run", "t", "")),
        again
    );
    // A request_id is not reusable for a different request.
    assert_eq!(
        code(
            &s,
            "signals.record",
            &replace(&signal("poll-1", "run", "t", ""), "external_id", "\"9002\"")
        ),
        "idempotency_conflict"
    );
}

#[test]
fn each_part_of_the_key_makes_a_distinct_notice() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    ended(&s, "other", "u");
    let base = signal("a", "run", "t", "");
    let mut sequences = vec![num(&s, &call(&s, "signals.record", &base), "$.sequence")];
    // A re-run of the same workflow run that now passes.
    let passed = replace(
        &replace(
            &replace(&base, "request_id", "\"b\""),
            "kind",
            "\"ci_passed\"",
        ),
        "conclusion",
        "\"success\"",
    );
    sequences.push(num(&s, &call(&s, "signals.record", &passed), "$.sequence"));
    // Another run id.
    let other_run = replace(
        &replace(&base, "request_id", "\"c\""),
        "external_id",
        "\"9002\"",
    );
    sequences.push(num(
        &s,
        &call(&s, "signals.record", &other_run),
        "$.sequence",
    ));
    // The local verdict source with the same id.
    let local = replace(
        &replace(
            &replace(&base, "request_id", "\"d\""),
            "source",
            "\"ci_local\"",
        ),
        "url",
        "<drop>",
    );
    let recorded = call(&s, "signals.record", &local);
    assert_eq!(text(&s, &recorded, "$.item.signal.source"), "ci_local");
    // No URL is stored as JSON null, never as an empty string.
    assert!(
        s.connection()
            .query_row(
                "SELECT json_extract(?1,'$.item.signal.url') IS NULL",
                [&recorded],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    sequences.push(num(&s, &recorded, "$.sequence"));
    // Another task's notice for the same external run.
    let task = replace(
        &replace(
            &replace(&base, "request_id", "\"e\""),
            "run_id",
            "\"other\"",
        ),
        "task_id",
        "\"u\"",
    );
    let theirs = call(&s, "signals.record", &task);
    assert_eq!(text(&s, &theirs, "$.item.task_id"), "u");
    sequences.push(num(&s, &theirs, "$.sequence"));
    let mut unique = sequences.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 5);
    let scoped = call(&s, "attention.list", r#"{"task_id":"t","filter":"all"}"#);
    assert_eq!(
        s.connection()
            .query_row(
                "SELECT count(*) FROM json_each(?1,'$.items') WHERE json_extract(value,'$.signal') IS NOT NULL",
                [&scoped],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        4
    );
}

#[test]
fn passed_and_cancelled_have_their_own_kinds_and_titles() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let passed = replace(
        &replace(&signal("p", "run", "t", ""), "kind", "\"ci_passed\""),
        "conclusion",
        "\"success\"",
    );
    let out = call(&s, "signals.record", &passed);
    assert_eq!(text(&s, &out, "$.item.kind"), "run_ci_passed");
    assert_eq!(
        text(&s, &out, "$.item.title"),
        "Coding agent branch passed CI"
    );
    let cancelled = replace(
        &replace(
            &replace(&signal("c", "run", "t", ""), "kind", "\"ci_cancelled\""),
            "conclusion",
            "\"cancelled\"",
        ),
        "external_id",
        "\"9003\"",
    );
    let out = call(&s, "signals.record", &cancelled);
    assert_eq!(text(&s, &out, "$.item.kind"), "run_ci_cancelled");
    assert_eq!(
        text(&s, &out, "$.item.title"),
        "Coding agent branch CI was cancelled"
    );
}

#[test]
fn an_unknown_kind_or_a_kind_that_contradicts_a_recorded_one_is_refused() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let seen = events(&s);
    for kind in [
        "\"ci_skipped\"",
        "\"run_ci_failed\"",
        "\"CI_FAILED\"",
        "\"\"",
        "7",
        "null",
    ] {
        let raw = replace(&signal("k", "run", "t", ""), "kind", kind);
        assert_eq!(code(&s, "signals.record", &raw), "invalid_input", "{kind}");
    }
    assert_eq!(events(&s), seen);
    call(&s, "signals.record", &signal("first", "run", "t", ""));
    let contradicting = replace(&signal("second", "run", "t", ""), "kind", "\"ci_passed\"");
    assert_eq!(
        code(&s, "signals.record", &contradicting),
        "idempotency_conflict"
    );
}

#[test]
fn every_field_is_bounded_and_a_refusal_records_nothing() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let seen = events(&s);
    let long = |n: usize| format!("\"{}\"", "x".repeat(n));
    let cases: Vec<(&str, String)> = vec![
        ("source", "\"gitlab\"".into()),
        ("source", "null".into()),
        ("external_id", "\"has space\"".into()),
        ("external_id", long(129)),
        ("external_id", "<drop>".into()),
        ("conclusion", "\"Failure\"".into()),
        ("conclusion", "\"time out\"".into()),
        ("conclusion", long(33)),
        ("conclusion", "\"\"".into()),
        ("workflow", "\"two\\nlines\"".into()),
        ("workflow", long(201)),
        ("workflow", "\"   \"".into()),
        ("workflow", "<drop>".into()),
        ("url", "\"http://example.com/run\"".into()),
        ("url", "\"javascript:alert(1)\"".into()),
        ("url", "\"https://example.com/a b\"".into()),
        ("url", format!("\"https://{}\"", "x".repeat(2050))),
        ("url", "5".into()),
        (
            "head_sha",
            "\"ABCDEF0123456789abcdef0123456789abcdef01\"".into(),
        ),
        ("head_sha", "\"abc123\"".into()),
        ("head_sha", format!("\"{}\"", "a".repeat(65))),
        ("head_sha", "<drop>".into()),
        ("run_id", "\"bad id\"".into()),
        ("task_id", "<drop>".into()),
        ("request_id", "<drop>".into()),
    ];
    for (field, value) in &cases {
        let raw = replace(&signal("bound", "run", "t", ""), field, value);
        assert_eq!(
            code(&s, "signals.record", &raw),
            "invalid_input",
            "{field}={value}"
        );
    }
    let extra = signal("extra", "run", "t", r#","permission_mode":"bypass""#);
    assert_eq!(code(&s, "signals.record", &extra), "invalid_input");
    let with_id = signal("extra", "run", "t", r#","id":"x","expected_revision":0"#);
    assert_eq!(code(&s, "signals.record", &with_id), "invalid_input");
    assert_eq!(events(&s), seen);
    // The edges that are allowed.
    let edge = replace(
        &replace(
            &replace(&signal("edge", "run", "t", ""), "workflow", &long(200)),
            "external_id",
            &format!("\"{}\"", "e".repeat(128)),
        ),
        "conclusion",
        &format!("\"{}\"", "c".repeat(32)),
    );
    let edge = replace(&edge, "head_sha", &format!("\"{}\"", "a".repeat(64)));
    call(&s, "signals.record", &edge);
}

#[test]
fn the_run_must_exist_belong_to_the_task_and_the_task_must_be_live() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let seen = events(&s);
    assert_eq!(
        code(&s, "signals.record", &signal("a", "missing", "t", "")),
        "not_found"
    );
    assert_eq!(
        code(&s, "signals.record", &signal("b", "run", "missing", "")),
        "not_found"
    );
    assert_eq!(
        code(&s, "signals.record", &signal("c", "run", "u", "")),
        "invalid_input"
    );
    assert_eq!(events(&s), seen);
    call(
        &s,
        "items.delete",
        r#"{"id":"t","request_id":"delete","expected_revision":1,"even_if_running":true}"#,
    );
    let seen = events(&s);
    assert_eq!(
        code(&s, "signals.record", &signal("d", "run", "t", "")),
        "not_found"
    );
    assert_eq!(events(&s), seen);
}

#[test]
fn a_task_holds_a_bounded_number_of_signals() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    for n in 0..2048 {
        let raw = replace(
            &replace(
                &signal(&format!("r{n}"), "run", "t", ""),
                "external_id",
                &format!("\"x{n}\""),
            ),
            "head_sha",
            &format!("\"{SHA}\""),
        );
        call(&s, "signals.record", &raw);
    }
    let seen = events(&s);
    let over = replace(
        &replace(&signal("over", "run", "t", ""), "external_id", "\"over\""),
        "head_sha",
        &format!("\"{SHA}\""),
    );
    assert_eq!(code(&s, "signals.record", &over), "capacity_reached");
    assert_eq!(events(&s), seen);
    // A repeat of what is already held is still answered, not refused.
    let held = replace(&signal("again", "run", "t", ""), "external_id", "\"x0\"");
    assert!(num(&s, &call(&s, "signals.record", &held), "$.recorded") == 0);
    // Another task is unaffected.
    ended(&s, "other", "u");
    call(&s, "signals.record", &signal("fresh", "other", "u", ""));
}

#[test]
fn the_notice_is_delivered_by_the_existing_native_notification_path() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    call(
        &s,
        "notifications.settings.put",
        r#"{"id":"profile","request_id":"settings","expected_revision":1,"enabled":true}"#,
    );
    let pending = |minute: i64| {
        call(
            &s,
            "notifications.pending.list",
            &format!(r#"{{"minute_of_day":{minute},"limit":10}}"#),
        )
    };
    assert_eq!(num(&s, &pending(720), "$.total"), 0);
    let out = call(&s, "signals.record", &signal("poll", "run", "t", ""));
    let id = text(&s, &out, "$.item.id");
    let page = pending(720);
    assert_eq!(num(&s, &page, "$.total"), 1);
    assert_eq!(text(&s, &page, "$.items[0].id"), id);
    assert_eq!(text(&s, &page, "$.items[0].kind"), "run_ci_failed");
    assert!(!page.contains("Private task"));
    let claim = call(
        &s,
        "notifications.claim",
        &format!(
            r#"{{"id":"{id}","request_id":"claim","expected_revision":0,"minute_of_day":720}}"#
        ),
    );
    assert_eq!(text(&s, &claim, "$.item.state"), "uncertain");
    assert_eq!(text(&s, &claim, "$.item.task_id"), "t");
    let delivery = call(
        &s,
        "notifications.delivery.get",
        &format!(r#"{{"id":"{id}"}}"#),
    );
    assert_eq!(text(&s, &delivery, "$.item.id"), id);
    assert_eq!(text(&s, &delivery, "$.item.task_id"), "t");
    assert_eq!(num(&s, &pending(720), "$.total"), 0);
    // A re-poll after delivery neither re-queues nor re-delivers it.
    call(&s, "signals.record", &signal("poll-again", "run", "t", ""));
    assert_eq!(num(&s, &pending(720), "$.total"), 0);
}

#[test]
fn a_result_for_a_live_attempt_goes_stale_when_the_attempt_moves_on() {
    let (s, _) = fixture();
    call(
        &s,
        "notifications.settings.put",
        r#"{"id":"profile","request_id":"settings","expected_revision":1,"enabled":true}"#,
    );
    prepare(&s, "live", "t");
    let out = call(&s, "signals.record", &signal("poll", "live", "t", ""));
    let id = text(&s, &out, "$.item.id");
    assert_eq!(text(&s, &out, "$.item.target_status"), "current");
    let get = format!(r#"{{"id":"{id}"}}"#);
    call(
        &s,
        "runs.claim",
        r#"{"id":"live","request_id":"claim-live","expected_revision":1,"owner_id":"host","session_id":"session"}"#,
    );
    assert_eq!(
        text(&s, &call(&s, "attention.get", &get), "$.item.target_status"),
        "changed"
    );
    let pending = call(
        &s,
        "notifications.pending.list",
        r#"{"minute_of_day":720,"limit":10}"#,
    );
    assert_eq!(num(&s, &pending, "$.total"), 0);
    // The re-poll still finds the notice instead of making a second one.
    let again = call(&s, "signals.record", &signal("poll-2", "live", "t", ""));
    assert!(num(&s, &again, "$.recorded") == 0);
    assert_eq!(text(&s, &again, "$.item.target_status"), "changed");
}

#[test]
fn a_task_edit_is_reported_and_a_signal_never_changes_the_task_or_the_run() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let task = call(&s, "items.get", r#"{"id":"t"}"#);
    let run = call(&s, "runs.get", r#"{"id":"run"}"#);
    let out = call(&s, "signals.record", &signal("poll", "run", "t", ""));
    assert_eq!(call(&s, "items.get", r#"{"id":"t"}"#), task);
    assert_eq!(call(&s, "runs.get", r#"{"id":"run"}"#), run);
    let id = text(&s, &out, "$.item.id");
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"edit","expected_revision":1,"title":"Revised","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    assert_eq!(
        text(
            &s,
            &call(&s, "attention.get", &format!(r#"{{"id":"{id}"}}"#)),
            "$.item.target_status"
        ),
        "task_changed"
    );
}

#[test]
fn the_notice_can_be_read_dismissed_and_does_not_disturb_other_kinds() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let out = call(&s, "signals.record", &signal("poll", "run", "t", ""));
    let id = text(&s, &out, "$.item.id");
    let read = call(
        &s,
        "attention.update",
        &format!(r#"{{"id":"{id}","request_id":"seen","expected_revision":1,"action":"read"}}"#),
    );
    assert_eq!(num(&s, &read, "$.item.revision"), 2);
    assert!(text(&s, &read, "$.item.signal.workflow") == "CI");
    // Other kinds carry no `signal` object.
    let finished = call(&s, "attention.list", r#"{"filter":"all"}"#);
    let exit = s
        .connection()
        .query_row(
            "SELECT value FROM json_each(?1,'$.items') WHERE json_extract(value,'$.kind')='run_failed'",
            [&finished],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    assert!(absent(&s, &exit, "$.signal"));
}

#[test]
fn a_failed_write_leaves_no_event_receipt_or_notice() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let seen = events(&s);
    let notices = num(
        &s,
        &call(&s, "attention.list", r#"{"filter":"all"}"#),
        "$.total",
    );
    s.connection()
        .execute_batch("CREATE TRIGGER attention_disk_failure BEFORE INSERT ON work_attention WHEN NEW.body LIKE '%run_ci_%' BEGIN SELECT RAISE(ABORT,'disk failure'); END;")
        .unwrap();
    assert_eq!(
        code(&s, "signals.record", &signal("poll", "run", "t", "")),
        "store_error"
    );
    assert_eq!(events(&s), seen);
    assert_eq!(
        num(
            &s,
            &call(&s, "attention.list", r#"{"filter":"all"}"#),
            "$.total"
        ),
        notices
    );
    let receipts: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_requests WHERE method='signals.record'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
    s.connection()
        .execute_batch("DROP TRIGGER attention_disk_failure")
        .unwrap();
    assert!(
        num(
            &s,
            &call(&s, "signals.record", &signal("poll", "run", "t", "")),
            "$.recorded"
        ) == 1
    );
}

#[test]
fn compaction_keeps_the_event_a_signal_notice_points_at() {
    let (s, clock) = fixture();
    ended(&s, "run", "t");
    let out = call(&s, "signals.record", &signal("poll", "run", "t", ""));
    let sequence = num(&s, &out, "$.sequence");
    clock.store(1000 + 90 * 86_400, Ordering::SeqCst);
    call(&s, "store.compact", r#"{"keep_events":1}"#);
    let kept: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_events WHERE sequence=?1",
            [sequence],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
    let again = call(
        &s,
        "signals.record",
        &signal("poll-much-later", "run", "t", ""),
    );
    assert!(num(&s, &again, "$.recorded") == 0);
}
