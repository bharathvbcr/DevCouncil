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
        r#"{{"request_id":"{request}","run_id":"{run}","task_id":"{task}","source":"github","external_id":"9001","kind":"ci_failed","conclusion":"failure","workflow":"CI","branch":"agent/t","url":"https://github.com/o/r/actions/runs/9001","head_sha":"{SHA}"{extra}}}"#
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
    assert_eq!(text(&s, &first, "$.item.signal.branch"), "agent/t");
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
        ("branch", "<drop>".into()),
        ("branch", "null".into()),
        ("branch", "\"   \"".into()),
        ("branch", "\"two\\nlines\"".into()),
        ("branch", long(1025)),
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
    let edge = replace(&edge, "branch", &long(1024));
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

fn enable_notifications(s: &Store) {
    call(
        s,
        "notifications.settings.put",
        r#"{"id":"profile","request_id":"settings","expected_revision":1,"enabled":true}"#,
    );
}
/// The ids of the CI notices native delivery would pick up now, newest first.
/// Other kinds a run raises (an exit notice) are not what these tests are about.
fn pending_ci(s: &Store) -> Vec<String> {
    let page = call(
        s,
        "notifications.pending.list",
        r#"{"minute_of_day":720,"limit":50}"#,
    );
    let mut stmt = s
        .connection()
        .prepare(
            "SELECT json_extract(value,'$.id') FROM json_each(?1,'$.items') WHERE json_extract(value,'$.kind') LIKE 'run_ci_%'",
        )
        .unwrap();
    stmt.query_map([&page], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
fn status(s: &Store, id: &str) -> String {
    text(
        s,
        &call(s, "attention.get", &format!(r#"{{"id":"{id}"}}"#)),
        "$.item.target_status",
    )
}
/// The same branch's next verdict: a new external run, optionally re-keyed.
fn verdict(
    request: &str,
    run: &str,
    task: &str,
    external_id: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut raw = replace(
        &signal(request, run, task, ""),
        "external_id",
        &format!("\"{external_id}\""),
    );
    for (key, value) in extra {
        raw = replace(&raw, key, value);
    }
    raw
}

#[test]
fn a_result_for_a_live_attempt_is_delivered_and_survives_the_attempts_own_writes() {
    let (s, _) = fixture();
    enable_notifications(&s);
    prepare(&s, "live", "t");
    let out = call(&s, "signals.record", &signal("poll", "live", "t", ""));
    let id = text(&s, &out, "$.item.id");
    assert_eq!(text(&s, &out, "$.item.target_status"), "current");
    // Recorded against a prepared attempt, it is deliverable at once.
    assert_eq!(pending_ci(&s), vec![id.clone()]);
    // The attempt moves on: claim and finish each write to the run.
    call(
        &s,
        "runs.claim",
        r#"{"id":"live","request_id":"claim-live","expected_revision":1,"owner_id":"host","session_id":"session"}"#,
    );
    assert_eq!(status(&s, &id), "current");
    assert_eq!(pending_ci(&s), vec![id.clone()]);
    call(
        &s,
        "runs.finish",
        r#"{"id":"live","request_id":"finish-live","expected_revision":2,"owner_id":"host","session_id":"session","outcome":"failed","reason":"Failed before spawn"}"#,
    );
    assert_eq!(status(&s, &id), "current");
    assert_eq!(pending_ci(&s), vec![id.clone()]);
    // A re-poll still finds the notice, and it is still current.
    let again = call(&s, "signals.record", &signal("poll-2", "live", "t", ""));
    assert!(num(&s, &again, "$.recorded") == 0);
    assert_eq!(text(&s, &again, "$.item.target_status"), "current");
    // And native delivery can claim it after all of that.
    let claim = call(
        &s,
        "notifications.claim",
        &format!(
            r#"{{"id":"{id}","request_id":"claim","expected_revision":0,"minute_of_day":720}}"#
        ),
    );
    assert_eq!(text(&s, &claim, "$.item.state"), "uncertain");
}

#[test]
fn a_signal_is_not_staled_by_unrelated_task_or_run_writes_and_only_a_deleted_task_ends_it() {
    let (s, _) = fixture();
    enable_notifications(&s);
    ended(&s, "run", "t");
    let out = call(&s, "signals.record", &signal("poll", "run", "t", ""));
    let id = text(&s, &out, "$.item.id");
    // Another attempt on the same task starting and ending is a run write.
    ended(&s, "later", "t");
    assert_eq!(status(&s, &id), "current");
    assert_eq!(pending_ci(&s), vec![id.clone()]);
    // A task edit used to read `task_changed` and block delivery.
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"edit","expected_revision":1,"title":"Revised","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    assert_eq!(status(&s, &id), "current");
    assert_eq!(pending_ci(&s), vec![id.clone()]);
    // A deleted task still wins: the notice must not outlive its task.
    let revision = num(
        &s,
        &call(&s, "items.get", r#"{"id":"t"}"#),
        "$.item.revision",
    );
    call(
        &s,
        "items.delete",
        &format!(
            r#"{{"id":"t","request_id":"delete","expected_revision":{revision},"even_if_running":true}}"#
        ),
    );
    assert_eq!(status(&s, &id), "task_deleted");
    assert!(pending_ci(&s).is_empty());
}

#[test]
fn a_newer_signal_for_the_same_task_source_workflow_and_branch_supersedes_the_older() {
    let (s, _) = fixture();
    enable_notifications(&s);
    ended(&s, "run", "t");
    let failed = call(&s, "signals.record", &signal("one", "run", "t", ""));
    let older = text(&s, &failed, "$.item.id");
    assert_eq!(pending_ci(&s), vec![older.clone()]);
    // The same branch is fixed and its next run passes.
    let passed = call(
        &s,
        "signals.record",
        &verdict(
            "two",
            "run",
            "t",
            "9002",
            &[("kind", "\"ci_passed\""), ("conclusion", "\"success\"")],
        ),
    );
    let newer = text(&s, &passed, "$.item.id");
    assert_eq!(text(&s, &passed, "$.item.target_status"), "current");
    assert_eq!(status(&s, &older), "changed");
    assert_eq!(status(&s, &newer), "current");
    // Only the newer verdict is pending; the older one stopped being so.
    assert_eq!(pending_ci(&s), vec![newer.clone()]);
    assert_eq!(
        code(
            &s,
            "notifications.claim",
            &format!(
                r#"{{"id":"{older}","request_id":"claim-old","expected_revision":0,"minute_of_day":720}}"#
            )
        ),
        "not_eligible"
    );
    // Superseding is read-time: the older notice and its facts are untouched.
    let kept = call(&s, "attention.get", &format!(r#"{{"id":"{older}"}}"#));
    assert_eq!(text(&s, &kept, "$.item.kind"), "run_ci_failed");
    assert_eq!(text(&s, &kept, "$.item.signal.conclusion"), "failure");
    assert_eq!(num(&s, &kept, "$.item.revision"), 1);
    // A third verdict supersedes the second too, and only the third is pending.
    let third = call(
        &s,
        "signals.record",
        &verdict("three", "run", "t", "9003", &[]),
    );
    let third = text(&s, &third, "$.item.id");
    assert_eq!(status(&s, &newer), "changed");
    assert_eq!(pending_ci(&s), vec![third]);
}

#[test]
fn signals_with_a_different_key_coexist() {
    let (s, _) = fixture();
    enable_notifications(&s);
    ended(&s, "run", "t");
    ended(&s, "other", "u");
    let base = call(&s, "signals.record", &signal("base", "run", "t", ""));
    let base = text(&s, &base, "$.item.id");
    let cases: Vec<(&str, String)> = vec![
        // Each differs from `base` in one part of the key and is a newer signal.
        (
            "branch",
            verdict("b", "run", "t", "1", &[("branch", "\"agent/other\"")]),
        ),
        (
            "workflow",
            verdict("w", "run", "t", "2", &[("workflow", "\"Lint\"")]),
        ),
        (
            "source",
            verdict(
                "s",
                "run",
                "t",
                "3",
                &[("source", "\"ci_local\""), ("url", "<drop>")],
            ),
        ),
        ("task", verdict("k", "other", "u", "4", &[])),
    ];
    let mut ids = vec![base.clone()];
    for (part, raw) in &cases {
        let out = call(&s, "signals.record", raw);
        let id = text(&s, &out, "$.item.id");
        assert_eq!(status(&s, &id), "current", "{part}");
        ids.push(id);
    }
    for id in &ids {
        assert_eq!(status(&s, id), "current");
    }
    let mut pending = pending_ci(&s);
    pending.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(pending, expected);
    // A newer verdict on one branch supersedes that branch only.
    let next = call(
        &s,
        "signals.record",
        &verdict("b2", "run", "t", "5", &[("branch", "\"agent/other\"")]),
    );
    let next = text(&s, &next, "$.item.id");
    assert_eq!(status(&s, &ids[1]), "changed");
    for id in [&ids[0], &ids[2], &ids[3], &ids[4], &next] {
        assert_eq!(status(&s, id), "current");
    }
    assert_eq!(pending_ci(&s).len(), 5);
}

#[test]
fn replaying_a_signal_neither_supersedes_nor_revives_anything() {
    let (s, _) = fixture();
    enable_notifications(&s);
    ended(&s, "run", "t");
    let first = call(&s, "signals.record", &signal("one", "run", "t", ""));
    let older = text(&s, &first, "$.item.id");
    let second = call(
        &s,
        "signals.record",
        &verdict("two", "run", "t", "9002", &[]),
    );
    let newer = text(&s, &second, "$.item.id");
    let seen = events(&s);
    // Re-polling the older result after the newer one must not make it newest.
    let replay = call(&s, "signals.record", &signal("one-again", "run", "t", ""));
    assert!(num(&s, &replay, "$.recorded") == 0);
    assert_eq!(text(&s, &replay, "$.item.id"), older);
    assert_eq!(text(&s, &replay, "$.item.target_status"), "changed");
    // Re-polling the newer one is the same no-op.
    let replay = call(
        &s,
        "signals.record",
        &verdict("two-again", "run", "t", "9002", &[]),
    );
    assert!(num(&s, &replay, "$.recorded") == 0);
    assert_eq!(text(&s, &replay, "$.item.id"), newer);
    assert_eq!(text(&s, &replay, "$.item.target_status"), "current");
    assert_eq!(events(&s), seen);
    assert_eq!(status(&s, &older), "changed");
    assert_eq!(status(&s, &newer), "current");
    assert_eq!(pending_ci(&s), vec![newer]);
}

#[test]
fn a_signal_never_changes_the_task_or_the_run() {
    let (s, _) = fixture();
    ended(&s, "run", "t");
    let task = call(&s, "items.get", r#"{"id":"t"}"#);
    let run = call(&s, "runs.get", r#"{"id":"run"}"#);
    call(&s, "signals.record", &signal("poll", "run", "t", ""));
    call(
        &s,
        "signals.record",
        &verdict("next", "run", "t", "9002", &[]),
    );
    assert_eq!(call(&s, "items.get", r#"{"id":"t"}"#), task);
    assert_eq!(call(&s, "runs.get", r#"{"id":"run"}"#), run);
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
